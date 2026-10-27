//! Native GEO / foundation editor presentation state.
//!
//! GeoState and its history remain owned by the shared Document. This module
//! holds only a source-bound edit draft, selections, filters and camera state.

use super::*;
use crate::geo_foundation_api as gf_api;
use crate::geo_foundation_render as gf_render;
use foundation_core::{FoundationId, FoundationLayout, FoundationPlacement, FoundationTier, RigidMatInput};
use geo_core::{
    Borehole, BoreholeId, GeoCoordinateFrame, GeoScenario, GeoSectionId, GeoSoilId, GeoSoilRecord, GeoState,
    GeoSnapshotSelection, GeoTriangle, GroundModel, GroundModelId, GroundScenarioId, IgeId, IgeRecord,
    InterpolationMethod, LayerContact, UnknownZone,
    ConfidenceClass, InterpretedSection, Parameter, ParameterCandidate, ParameterMetadata,
    ParameterUnit, SoilParameters, SourceId, SourceRecord, WaterObservation,
    GeoCommand,
};
use makepad_widgets::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use crate::drafting_canvas::{
    AnalysisCanvasLayer, CanvasAction, CanvasCameraChange, CanvasFrame, CanvasInput,
    DraftingCanvasWidgetRefExt, PointerButton, PointerPhase,
};
use crate::viewport::{CadViewportWidgetRefExt};
use drafting_core::{Camera2D, Point2 as DraftPoint};
use viewer_core::{ObjectId, Viewer, ViewerEvent, Viewport as CoreViewport};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.GeoFoundationWorkspace = SolidView{
        width: Fill height: Fill flow: Right visible: false
        draw_bg.color: #x111922

        ScrollYView{
            width: 352 height: Fill flow: Down spacing: 7 padding: 10
            draw_bg.color: #x17212b
            H3{width: Fill text: "GEO / Foundation" draw_text.color: #xe5edf6}
            Label{width: Fill height: Fit text: "Source-bound geological model · explicit provenance" draw_text.color: #x9fb3c7}
            View{width: Fill height: 32 flow: Right spacing: 5
                gf_load_demo := Button{width: Fill text: "Load shared synthetic example"}
                gf_reload := Button{width: Fill text: "Reload"}
                gf_apply := Button{width: Fill text: "Apply record"}
            }
            View{width: Fill height: 32 flow: Right spacing: 5
                gf_save_snapshot := Button{width: Fill text: "Save selected snapshot"}
                gf_snapshot_status := Label{width: Fill height: Fit text: "No snapshot" draw_text.color: #xaec0d1}
            }
            Label{width: Fill text: "Saved immutable snapshot · selected closure only" draw_text.color: #xb6c8d8}
            gf_snapshot_select := DropDown{width: Fill labels: ["No saved snapshots"]}
            gf_snapshot_id := TextInput{width: Fill height: 30 empty_text: "New snapshot ID for explicit Save"}
            gf_binding_label := Label{width: Fill height: Fit text: "No Document binding" draw_text.color: #x9badbe}
            gf_status := Label{width: Fill height: Fit text: "MODEL_INCOMPLETE · no Geo records" draw_text.color: #xf2c47c}
            gf_attempt_status := Label{width: Fill height: Fit text: "" draw_text.color: #xe4b77a}

            Hr{}
            Label{width: Fill text: "Selected model" draw_text.color: #xb6c8d8}
            gf_model_select := DropDown{width: Fill labels: ["No stored ground model"]}
            View{width: Fill height: 30 flow: Right spacing: 5
                gf_new_model := Button{width: Fill text: "+ Model"}
                gf_new_scenario := Button{width: Fill text: "+ Scenario"}
            }
            Label{width: Fill text: "Scenario · water state is explicit" draw_text.color: #xb6c8d8}
            gf_scenario_select := DropDown{width: Fill labels: ["No stored scenario"]}

            Hr{}
            View{width: Fill height: 32 flow: Right spacing: 4
                gf_tab_sources := Button{width: Fill text: "Sources"}
                gf_tab_soils := Button{width: Fill text: "Soils"}
                gf_tab_iges := Button{width: Fill text: "IGEs"}
            }
            View{width: Fill height: 32 flow: Right spacing: 4
                gf_tab_boreholes := Button{width: Fill text: "Boreholes"}
                gf_tab_model := Button{width: Fill text: "TIN"}
                gf_tab_water := Button{width: Fill text: "Water"}
            }
            View{width: Fill height: 32 flow: Right spacing: 4
                gf_tab_sections := Button{width: Fill text: "Sections"}
                gf_tab_contacts := Button{width: Fill text: "Contacts"}
                gf_tab_unknown := Button{width: Fill text: "Unknown zones"}
            }

            gf_sources_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "Source record" draw_text.color: #xd7e3ee}
                gf_source_select := DropDown{width: Fill labels: ["No stored sources"]}
                gf_new_source := Button{width: Fill text: "+ Source"}
                gf_source_id := TextInput{width: Fill height: 30 empty_text: "Source ID"}
                gf_source_title := TextInput{width: Fill height: 30 empty_text: "Title"}
                gf_source_origin := DropDown{width: Fill labels: ["Choose origin", "Synthetic", "Measured", "Interpreted", "Imported legacy"]}
                gf_source_reference := TextInput{width: Fill height: 54 is_multiline: true empty_text: "Reference / provenance"}
                gf_source_sha256 := TextInput{width: Fill height: 30 empty_text: "Content SHA-256 · optional"}
                gf_source_observed_at := TextInput{width: Fill height: 30 empty_text: "Observed at · optional"}
            }

            gf_soils_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "Editable GeoSoilRecord registry" draw_text.color: #xd7e3ee}
                gf_soil_select := DropDown{width: Fill labels: ["No stored GeoSoilRecord"]}
                gf_new_soil := Button{width: Fill text: "+ GeoSoilRecord"}
                gf_soil_id := TextInput{width: Fill height: 30 empty_text: "GeoSoilId · validated ID"}
                gf_soil_name := TextInput{width: Fill height: 30 empty_text: "Soil name"}
                Label{width: Fill text: "Parameter values keep their own source IDs. KNOWN is not reviewed." draw_text.color: #xe4b77a}
                gf_soil_parameter := DropDown{width: Fill labels: ["Unit weight · kN/m³", "Cohesion · kPa", "Friction angle · degrees", "Elastic modulus · kPa", "Poisson ratio"]}
                gf_parameter_state := DropDown{width: Fill labels: ["MISSING", "KNOWN", "CONFLICTING"]}
                gf_parameter_value := TextInput{width: Fill height: 30 empty_text: "Value · required only for KNOWN"}
                gf_parameter_missing_reason := TextInput{width: Fill height: 42 empty_text: "Missing reason"}
                gf_parameter_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Source IDs · one per line"}
                gf_parameter_candidates := TextInput{width: Fill height: 110 is_multiline: true empty_text: "Conflicting candidates JSON · value and source_ids"}
                gf_parameter_status_summary := TextInput{width: Fill height: 100 is_multiline: true is_read_only: true text: "Parameter status summary"}
            }

            gf_iges_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "IGE registry" draw_text.color: #xd7e3ee}
                gf_ige_select := DropDown{width: Fill labels: ["No stored IGEs"]}
                gf_new_ige := Button{width: Fill text: "+ IGE"}
                gf_ige_id := TextInput{width: Fill height: 30 empty_text: "IgeId · validated ID"}
                gf_ige_name := TextInput{width: Fill height: 30 empty_text: "IGE name"}
                gf_ige_soil := DropDown{width: Fill labels: ["No stored GeoSoilRecord"]}
                gf_ige_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Source IDs · one per line"}
            }

            gf_boreholes_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "Boreholes · collar XYZ, metres" draw_text.color: #xd7e3ee}
                gf_borehole_select := DropDown{width: Fill labels: ["No stored boreholes"]}
                gf_new_borehole := Button{width: Fill text: "+ Borehole"}
                gf_borehole_id := TextInput{width: Fill height: 30 empty_text: "BoreholeId · validated ID"}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_collar_x := TextInput{width: Fill height: 30 empty_text: "X, m"}
                    gf_collar_y := TextInput{width: Fill height: 30 empty_text: "Y, m"}
                    gf_collar_z := TextInput{width: Fill height: 30 empty_text: "Z, m"}
                }
                gf_borehole_collar_vertical_datum := TextInput{width: Fill height: 30 empty_text: "Collar vertical datum · explicit; blank remains unspecified"}
                gf_borehole_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Borehole source IDs · one per line"}
                gf_borehole_total_depth_state := DropDown{width: Fill labels: ["MISSING", "KNOWN", "CONFLICTING"]}
                gf_borehole_total_depth := TextInput{width: Fill height: 30 empty_text: "Total drilled depth, m"}
                gf_borehole_total_depth_reason := TextInput{width: Fill height: 38 empty_text: "Missing depth reason"}
                gf_borehole_total_depth_sources := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Total depth source IDs"}
                gf_borehole_date := TextInput{width: Fill height: 30 empty_text: "Drilling date · optional"}
                gf_borehole_crs := TextInput{width: Fill height: 30 empty_text: "Borehole CRS · optional"}
                gf_borehole_depth_datum := TextInput{width: Fill height: 30 empty_text: "Depth datum"}
                gf_borehole_survey_sources := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Survey source IDs"}
                gf_borehole_evidence_sources := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Evidence source IDs"}
                Label{width: Fill text: "Intervals are ordered depth ranges with explicit IGE and source IDs." draw_text.color: #xb6c8d8}
                gf_interval_select := DropDown{width: Fill labels: ["No intervals"]}
                gf_interval_rows := TextInput{width: Fill height: 92 is_multiline: true is_read_only: true text: "No borehole intervals"}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_interval_top := TextInput{width: Fill height: 30 empty_text: "Top depth, m"}
                    gf_interval_bottom := TextInput{width: Fill height: 30 empty_text: "Bottom depth, m"}
                gf_interval_id := TextInput{width: Fill height: 30 empty_text: "BoreholeIntervalId · validated"}
                }
                gf_interval_ige := DropDown{width: Fill labels: ["No stored IGEs"]}
                gf_interval_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Interval source IDs · one per line"}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_interval_add := Button{width: Fill text: "+ Interval"}
                    gf_interval_apply := Button{width: Fill text: "Apply interval"}
                }
            }

            gf_model_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "Ground model · explicit TIN only" draw_text.color: #xd7e3ee}
                gf_ground_model_id := TextInput{width: Fill height: 30 empty_text: "GroundModelId · validated ID"}
                gf_ground_model_name := TextInput{width: Fill height: 30 empty_text: "Model name"}
                gf_frame_name := TextInput{width: Fill height: 30 empty_text: "Coordinate frame name"}
                gf_frame_crs := TextInput{width: Fill height: 30 empty_text: "CRS · optional"}
                gf_frame_datum := TextInput{width: Fill height: 30 empty_text: "Vertical datum · optional"}
                gf_model_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Model source IDs · one per line"}
                gf_model_boreholes := TextInput{width: Fill height: 70 is_multiline: true is_read_only: true text: "No borehole IDs in model"}
                gf_model_borehole_select := DropDown{width: Fill labels: ["No stored boreholes"]}
                gf_model_borehole_add := Button{width: Fill text: "Add selected borehole to model"}
                gf_model_borehole_remove := Button{width: Fill text: "Remove selected borehole from model"}
                gf_allow_pinch := CheckBox{width: Fill height: 28 text: "Allow explicit pinch" active: false}
                Label{width: Fill text: "Interpolation: explicit TIN · piecewise planar · inside coverage only" draw_text.color: #xaec0d1}
                Hr{}
                Label{width: Fill text: "Triangle connectivity · no generated triangulation" draw_text.color: #xb6c8d8}
                gf_triangle_select := DropDown{width: Fill labels: ["No stored triangles"]}
                gf_new_triangle := Button{width: Fill text: "+ Triangle"}
                gf_triangle_id := TextInput{width: Fill height: 30 empty_text: "TriangleId · validated ID"}
                gf_triangle_vertex_a := DropDown{width: Fill labels: ["No stored boreholes"]}
                gf_triangle_vertex_b := DropDown{width: Fill labels: ["No stored boreholes"]}
                gf_triangle_vertex_c := DropDown{width: Fill labels: ["No stored boreholes"]}
                gf_triangle_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Triangle source IDs · one per line"}
            }

            gf_contacts_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "Ordered layer contacts" draw_text.color: #xd7e3ee}
                gf_contact_select := DropDown{width: Fill labels: ["No stored contacts"]}
                gf_new_contact := Button{width: Fill text: "+ Contact horizon"}
                gf_contact_id := TextInput{width: Fill height: 30 empty_text: "HorizonId · validated ID"}
                gf_contact_ige := DropDown{width: Fill labels: ["No stored IGEs"]}
                gf_contact_borehole := DropDown{width: Fill labels: ["No model boreholes"]}
                gf_contact_vertical_datum := TextInput{width: Fill height: 30 empty_text: "Contact vertical datum · explicit; blank remains unspecified"}
                Label{width: Fill text: "Contact elevation at selected borehole · datum must match model for numeric reference checks; absence stays MISSING" draw_text.color: #xe4b77a}
                gf_contact_parameter_state := DropDown{width: Fill labels: ["MISSING", "KNOWN", "CONFLICTING"]}
                gf_contact_elevation := TextInput{width: Fill height: 30 empty_text: "Absolute elevation, m"}
                gf_contact_missing_reason := TextInput{width: Fill height: 42 empty_text: "Missing reason"}
                gf_contact_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Source IDs · one per line"}
                gf_contact_candidates := TextInput{width: Fill height: 100 is_multiline: true empty_text: "Conflicting candidates JSON"}
                gf_contact_summary := TextInput{width: Fill height: 100 is_multiline: true is_read_only: true text: "Contact map summary"}
            }

            gf_unknown_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "Explicit unknown zones" draw_text.color: #xd7e3ee}
                gf_unknown_select := DropDown{width: Fill labels: ["No unknown zones"]}
                gf_new_unknown := Button{width: Fill text: "+ Unknown zone"}
                gf_unknown_polygon := TextInput{width: Fill height: 120 is_multiline: true empty_text: "XY polygon vertices · one pair per line, metres"}
                gf_unknown_reason := TextInput{width: Fill height: 56 is_multiline: true empty_text: "Reason for unknown coverage"}
                gf_unknown_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Source IDs · one per line"}
                Label{width: Fill text: "Unknown zones and missing contacts stay unknown; no extrapolation." draw_text.color: #xe4b77a}
            }

            gf_water_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "Scenario water state" draw_text.color: #xd7e3ee}
                gf_scenario_id := TextInput{width: Fill height: 30 empty_text: "GroundScenarioId · validated ID"}
                gf_scenario_name := TextInput{width: Fill height: 30 empty_text: "Scenario name"}
                gf_scenario_model := DropDown{width: Fill labels: ["No stored ground models"]}
                gf_water_kind := DropDown{width: Fill labels: ["Unknown", "Explicit dry assumption", "Observed water levels"]}
                gf_scenario_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Scenario source IDs · one per line"}
                gf_water_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "ExplicitDry source IDs"}
                gf_water_observation_select := DropDown{width: Fill labels: ["No stored water observations"]}
                gf_water_observation_ids_text := TextInput{width: Fill height: 64 is_multiline: true empty_text: "Observation IDs in this scenario · one per line"}
                gf_new_water_observation := Button{width: Fill text: "+ Water observation"}
                gf_water_observation_id := TextInput{width: Fill height: 30 empty_text: "WaterObservationId · validated ID"}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_water_x := TextInput{width: Fill height: 30 empty_text: "X, m"}
                    gf_water_y := TextInput{width: Fill height: 30 empty_text: "Y, m"}
                }
                gf_water_observed_at := TextInput{width: Fill height: 30 empty_text: "Observed at · optional"}
                gf_water_elevation_state := DropDown{width: Fill labels: ["MISSING", "KNOWN", "CONFLICTING"]}
                gf_water_elevation := TextInput{width: Fill height: 30 empty_text: "Absolute elevation, m"}
                gf_water_elevation_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Source IDs · one per line"}
                Label{width: Fill text: "UNKNOWN is not converted to dry. ExplicitDry remains a named assumption with sources." draw_text.color: #xe4b77a}
            }

            gf_sections_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "Interpreted section alignment" draw_text.color: #xd7e3ee}
                gf_section_select := DropDown{width: Fill labels: ["No stored interpreted sections"]}
                gf_new_section := Button{width: Fill text: "+ Section"}
                gf_section_id := TextInput{width: Fill height: 30 empty_text: "GeoSectionId · validated ID"}
                gf_section_model := DropDown{width: Fill labels: ["No stored ground models"]}
                gf_section_alignment := TextInput{width: Fill height: 130 is_multiline: true empty_text: "Ordered XY alignment · one pair per line, metres"}
                gf_section_source_ids := TextInput{width: Fill height: 44 is_multiline: true empty_text: "Source IDs · one per line"}
            }
            gf_foundation_host := View{width: Fill height: Fit flow: Down spacing: 6 visible: false
                H4{width: Fill text: "Foundation placement geometry · no bearing or settlement acceptance" draw_text.color: #xd7e3ee}
                gf_layout_status := TextInput{width: Fill height: 88 is_multiline: true is_read_only: true text: "No saved foundation layout"}
                gf_layout_id := TextInput{width: Fill height: 30 empty_text: "FoundationLayout ID"}
                gf_placement_select := DropDown{width: Fill labels: ["No stored placements"]}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_placement_add := Button{width: Fill text: "+ Placement"}
                    gf_placement_delete := Button{width: Fill text: "Delete selected"}
                }
                gf_placement_id := TextInput{width: Fill height: 30 empty_text: "FoundationId · stable ID"}
                gf_placement_kind := DropDown{width: Fill labels: ["Raft", "Strip", "Isolated", "Stepped"]}
                Label{width: Fill text: "Coordinates are global XYZ in metres; Z is lower-face elevation." draw_text.color: #xb6c8d8}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_placement_x := TextInput{width: Fill height: 30 empty_text: "Center X, m"}
                    gf_placement_y := TextInput{width: Fill height: 30 empty_text: "Center Y, m"}
                    gf_placement_z := TextInput{width: Fill height: 30 empty_text: "Base Z, m"}
                }
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_placement_length_x := TextInput{width: Fill height: 30 empty_text: "Length X, m"}
                    gf_placement_width_y := TextInput{width: Fill height: 30 empty_text: "Width Y, m"}
                    gf_placement_thickness := TextInput{width: Fill height: 30 empty_text: "Thickness, m"}
                }
                gf_placement_strip_length := TextInput{width: Fill height: 30 empty_text: "Strip length, m"}
                gf_placement_strip_width := TextInput{width: Fill height: 30 empty_text: "Strip width, m"}
                gf_placement_rotation := TextInput{width: Fill height: 30 empty_text: "Strip rotation, rad"}
                Label{width: Fill text: "Stepped placement tier · lower face elevation in global Z" draw_text.color: #xb6c8d8}
                gf_tier_select := DropDown{width: Fill labels: ["No tiers"]}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_tier_add := Button{width: Fill text: "+ Tier"}
                    gf_tier_delete := Button{width: Fill text: "Delete tier"}
                }
                gf_tier_id := TextInput{width: Fill height: 30 empty_text: "Tier ID"}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_tier_x := TextInput{width: Fill height: 30 empty_text: "Center X, m"}
                    gf_tier_y := TextInput{width: Fill height: 30 empty_text: "Center Y, m"}
                    gf_tier_z := TextInput{width: Fill height: 30 empty_text: "Base Z, m"}
                }
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_tier_length_x := TextInput{width: Fill height: 30 empty_text: "Length X, m"}
                    gf_tier_width_y := TextInput{width: Fill height: 30 empty_text: "Width Y, m"}
                    gf_tier_thickness := TextInput{width: Fill height: 30 empty_text: "Thickness, m"}
                }
                Label{width: Fill text: "Ground link for selected placement · explicit stored model and water scenario" draw_text.color: #xb6c8d8}
                gf_ground_link_model := DropDown{width: Fill labels: ["No model selected"]}
                gf_ground_link_scenario := DropDown{width: Fill labels: ["No scenario selected"]}
                gf_ground_link_apply := Button{width: Fill text: "Apply or clear placement GEO link"}
                gf_ground_map_build := Button{width: Fill text: "Sample source-bound raft GEO map · 40 × 48"}
                gf_ground_map_status := TextInput{width: Fill height: 72 is_multiline: true is_read_only: true text: "No midpoint map requested. Sampling is not continuous coverage."}
                Label{width: Fill text: "Authored synthetic bedding declaration · no Geo E/ν conversion; source must belong to linked snapshot" draw_text.color: #xe4b77a}
                gf_bedding_source_select := DropDown{width: Fill labels: ["Choose an existing GEO source ID"]}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_bedding_source_add := Button{width: Fill text: "Add selected bedding source"}
                    gf_bedding_source_remove := Button{width: Fill text: "Remove selected bedding source"}
                    gf_bedding_clear := Button{width: Fill text: "Clear bedding declaration"}
                }
                gf_bedding_source_ids := TextInput{width: Fill height: 56 is_multiline: true is_read_only: true text: "No authored bedding sources selected"}
                gf_layout_save := Button{width: Fill text: "Save layout to Document"}
                Label{width: Fill text: "Stored analysis snapshot · explicitly matched GEO/layout dependencies" draw_text.color: #xb6c8d8}
                gf_foundation_snapshot_select := DropDown{width: Fill labels: ["No foundation analysis snapshots"]}
                Label{width: Fill text: "Conditional numerical analysis · Raft + source-linked ExplicitDry context only" draw_text.color: #xe4b77a}
                gf_analysis_load_mode := DropDown{width: Fill labels: ["Manual vertical resultant", "Saved configured reactions"]}
                gf_control_template_select := DropDown{width: Fill labels: ["Choose existing numerical control template"]}
                gf_configured_load_select := DropDown{width: Fill labels: ["Choose saved configured load source"]}
                Label{width: Fill text: "Manual vertical-resultant controls · N and moments are explicit user inputs; k is independent of Geo E/ν." draw_text.color: #xe4b77a}
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_manual_n_kn := TextInput{width: Fill height: 30 empty_text: "N, kN"}
                    gf_manual_mx_knm := TextInput{width: Fill height: 30 empty_text: "Physical Mx, kN·m"}
                    gf_manual_my_knm := TextInput{width: Fill height: 30 empty_text: "Physical My, kN·m"}
                }
                View{width: Fill height: 30 flow: Right spacing: 5
                    gf_manual_k_kn_m3 := TextInput{width: Fill height: 30 empty_text: "Manual uniform k, kN/m³"}
                    gf_manual_gap_m := TextInput{width: Fill height: 30 empty_text: "Gap, m"}
                    gf_manual_own_weight := CheckBox{width: Fit text: "Add raft self-weight once" active: false}
                }
                gf_foundation_source_reference := TextInput{width: Fill height: 30 empty_text: "Source / assumption reference · required"}
                gf_foundation_bedding_note := TextInput{width: Fill height: 56 is_multiline: true empty_text: "Manual bedding assumption note · required"}
                Label{width: Fill text: "Advanced full RigidMatInput JSON fallback · only an explicitly chosen existing template supplies remaining geometry/mesh/metadata." draw_text.color: #xb6c8d8}
                gf_foundation_input_json := TextInput{width: Fill height: 150 is_multiline: true empty_text: "Optional advanced JSON. No default soil, load, or site values are inserted."}
                gf_foundation_analyze := Button{width: Fill text: "Analyze explicit Raft input"}
                Label{width: Fill text: "Synthetic soil and uniform k remain explicit manual assumptions. Result is conditional; no bearing, settlement-code or ULS/SLS acceptance." draw_text.color: #xe4b77a}
                Label{width: Fill text: "No placement means no foundation geometry. This panel does not certify bearing, sliding, uplift, or settlement." draw_text.color: #xe4b77a}
            }
        }

        View{width: Fill height: Fill flow: Down
            SolidView{width: Fill height: 36 flow: Right spacing: 4 padding: 4 draw_bg.color: #x202a34
                gf_view_plan := Button{width: Fill text: "Ground plan"}
                gf_view_section := Button{width: Fill text: "Section"}
                gf_view_3d := Button{width: Fill text: "Ground 3D"}
                gf_view_foundation_plan := Button{width: Fill text: "Foundation plan"}
            }
            SolidView{width: Fill height: 36 flow: Right spacing: 4 padding: 4 draw_bg.color: #x202a34
                gf_view_foundation_3d := Button{width: Fill text: "Foundation 3D"}
                gf_view_pressure := Button{width: Fill text: "Pressure"}
                gf_view_settlement := Button{width: Fill text: "Settlement"}
                gf_fit := Button{width: Fit text: "Fit"}
            }
            View{width: Fill height: 34 flow: Right spacing: 5 align: Align{y:0.5} padding: Inset{left: 8 right: 8}
                gf_show_unknown := CheckBox{width: Fit text: "Show unknown zones" active: true}
                gf_show_terrain := CheckBox{width: Fit text: "Terrain" active: true}
                gf_show_contacts := CheckBox{width: Fit text: "Contacts" active: true}
                gf_show_boreholes := CheckBox{width: Fit text: "Boreholes" active: true}
                gf_show_water := CheckBox{width: Fit text: "Water" active: true}
                gf_show_foundations := CheckBox{width: Fit text: "Foundations" active: true}
            }
            View{width: Fill height: 32 flow: Right spacing: 5 align: Align{y:0.5} padding: Inset{left: 8 right: 8}
                Label{width: Fit text: "IGE display" draw_text.color: #xb6c8d8}
                gf_visible_ige := DropDown{width: 260 labels: ["All IGEs"]}
                Label{width: Fit text: "Display filter only · source queries unchanged" draw_text.color: #x9fb3c7}
            }
            View{width: Fill height: 34 flow: Right spacing: 5 align: Align{y:0.5} padding: Inset{left: 8 right: 8}
                Label{width: Fit text: "Section sample spacing" draw_text.color: #xb6c8d8}
                gf_section_spacing := TextInput{width: 120 height: 30 text: "1.0" empty_text: "max spacing, m"}
                Label{width: Fit text: "m · bounded query" draw_text.color: #x9fb3c7}
            }
            gf_view_title := Label{width: Fill height: Fit text: "Plan · no current model" draw_text.color: #xc4d2df}
            gf_render_status := Label{width: Fill height: Fit text: "MODEL_INCOMPLETE · no typed ground projection" draw_text.color: #xf2c47c}
            gf_engineering_hold := Label{width: Fill height: Fit text: "GEO geometry/query only · foundation pressure/settlement conditional · bearing and design checks HOLD" draw_text.color: #xe4b77a}
            gf_canvas_host := View{width: Fill height: Fill visible: true
                gf_canvas := mod.widgets.DraftingCanvas{}
            }
            gf_scene_host := View{width: Fill height: Fill visible: false
                gf_viewport := mod.widgets.CadViewport{}
            }
        }

        ScrollYView{width: 348 height: Fill flow: Down spacing: 7 padding: 10
            draw_bg.color: #x17212b
            H3{width: Fill text: "Inspector / source status" draw_text.color: #xe5edf6}
            gf_selection_label := Label{width: Fill height: Fit text: "No Geo record selected" draw_text.color: #xc4d2df}
            gf_inspector_text := TextInput{width: Fill height: 210 is_multiline: true is_read_only: true text: "Record details and provenance appear here."}
            Hr{}
            Label{width: Fill text: "Current report / provenance" draw_text.color: #xb6c8d8}
            gf_report_text := TextInput{width: Fill height: 340 is_multiline: true is_read_only: true text: "No current report."}
            Hr{}
            Label{width: Fill text: "Derived Geo query · not a solver result" draw_text.color: #xb6c8d8}
            gf_query_kind := DropDown{width: Fill labels: ["Column at XY", "Section samples", "Layer volumes"]}
            View{width: Fill height: 30 flow: Right spacing: 5
                gf_query_x := TextInput{width: Fill height: 30 empty_text: "X, m"}
                gf_query_y := TextInput{width: Fill height: 30 empty_text: "Y, m"}
            }
            gf_query_spacing := TextInput{width: Fill height: 30 empty_text: "Section sample spacing, m"}
            gf_query_run := Button{width: Fill text: "Query selected snapshot"}
            gf_query_text := TextInput{width: Fill height: 230 is_multiline: true is_read_only: true text: "No query result."}
            Label{width: Fill height: Fit text: "TIN interpolation is piecewise planar inside explicit coverage only. Unknown/conflicting values remain explicit." draw_text.color: #xe4b77a}
        }
    }
}

pub(super) use crate::geo_foundation_render::GeoViewMode as GeoFoundationView;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum GeoFoundationTab {
    Sources,
    #[default]
    Soils,
    Iges,
    Boreholes,
    Model,
    Contacts,
    Water,
    Sections,
    UnknownZones,
}

#[derive(Clone, Debug)]
pub(super) enum GeoFoundationDraft {
    Source(SourceRecord),
    Soil(GeoSoilRecord),
    Ige(IgeRecord),
    Borehole(Borehole),
    Model(GroundModel),
    Scenario(GeoScenario),
    WaterObservation(WaterObservation),
    Section(InterpretedSection),
    UnknownZone(UnknownZone),
}

pub(super) const GEO_FOUNDATION_ALIASES: &[&str] = &[
    "GeoFoundationWorkspace", "gf_workspace", "workspace_geo_foundation", "gf_load_demo", "gf_reload", "gf_apply", "gf_save_snapshot", "gf_snapshot_status",
    "gf_binding_label", "gf_status", "gf_attempt_status", "gf_snapshot_select", "gf_snapshot_id", "gf_model_select", "gf_new_model",
    "gf_new_scenario", "gf_scenario_select", "gf_tab_sources", "gf_tab_soils", "gf_tab_iges",
    "gf_tab_boreholes", "gf_tab_model", "gf_tab_water", "gf_tab_sections", "gf_tab_contacts",
    "gf_tab_unknown", "gf_sources_host", "gf_source_select", "gf_new_source", "gf_source_id",
    "gf_source_title", "gf_source_origin", "gf_source_reference", "gf_source_sha256",
    "gf_source_observed_at", "gf_soils_host", "gf_soil_select", "gf_new_soil", "gf_soil_id",
    "gf_soil_name", "gf_soil_parameter", "gf_parameter_state", "gf_parameter_value",
    "gf_parameter_missing_reason", "gf_parameter_source_ids", "gf_parameter_candidates",
    "gf_parameter_status_summary", "gf_iges_host", "gf_ige_select", "gf_new_ige", "gf_ige_id",
    "gf_ige_name", "gf_ige_soil", "gf_ige_source_ids", "gf_boreholes_host", "gf_borehole_select",
    "gf_new_borehole", "gf_borehole_id", "gf_collar_x", "gf_collar_y", "gf_collar_z", "gf_borehole_collar_vertical_datum",
    "gf_borehole_source_ids", "gf_borehole_total_depth_state", "gf_borehole_total_depth",
    "gf_borehole_total_depth_reason", "gf_borehole_total_depth_sources", "gf_borehole_date",
    "gf_borehole_crs", "gf_borehole_depth_datum", "gf_borehole_survey_sources", "gf_borehole_evidence_sources",
    "gf_interval_select", "gf_interval_rows", "gf_interval_top",
    "gf_interval_bottom", "gf_interval_id", "gf_interval_ige", "gf_interval_source_ids", "gf_interval_add",
    "gf_interval_apply", "gf_model_host", "gf_ground_model_id", "gf_ground_model_name",
    "gf_frame_name", "gf_frame_crs", "gf_frame_datum", "gf_model_source_ids", "gf_model_boreholes",
    "gf_model_borehole_select", "gf_model_borehole_add", "gf_model_borehole_remove", "gf_allow_pinch", "gf_triangle_select",
    "gf_new_triangle", "gf_triangle_id", "gf_triangle_vertex_a", "gf_triangle_vertex_b",
    "gf_triangle_vertex_c", "gf_triangle_source_ids", "gf_contacts_host", "gf_contact_select",
    "gf_new_contact", "gf_contact_id", "gf_contact_ige", "gf_contact_borehole", "gf_contact_vertical_datum", "gf_contact_parameter_state",
    "gf_contact_elevation", "gf_contact_missing_reason", "gf_contact_source_ids",
    "gf_contact_candidates", "gf_contact_summary", "gf_unknown_host", "gf_unknown_select",
    "gf_new_unknown", "gf_unknown_polygon", "gf_unknown_reason", "gf_unknown_source_ids", "gf_water_host",
    "gf_scenario_id", "gf_scenario_name", "gf_scenario_model", "gf_water_kind", "gf_scenario_source_ids",
    "gf_water_source_ids", "gf_water_observation_select", "gf_water_observation_ids_text", "gf_new_water_observation",
    "gf_water_observation_id", "gf_water_x", "gf_water_y", "gf_water_observed_at",
    "gf_water_elevation_state", "gf_water_elevation", "gf_water_elevation_source_ids",
    "gf_sections_host", "gf_section_select", "gf_new_section", "gf_section_id", "gf_section_model",
    "gf_section_alignment", "gf_section_source_ids", "gf_foundation_host", "gf_layout_status",
    "gf_layout_id", "gf_placement_select", "gf_placement_add", "gf_placement_delete",
    "gf_placement_id", "gf_placement_kind", "gf_placement_x", "gf_placement_y", "gf_placement_z",
    "gf_placement_length_x", "gf_placement_width_y", "gf_placement_thickness", "gf_placement_strip_length",
    "gf_placement_strip_width", "gf_placement_rotation", "gf_tier_select", "gf_tier_add",
    "gf_tier_delete", "gf_tier_id", "gf_tier_x", "gf_tier_y", "gf_tier_z", "gf_tier_length_x",
    "gf_tier_width_y", "gf_tier_thickness", "gf_ground_link_model", "gf_ground_link_scenario",
    "gf_ground_link_apply", "gf_ground_map_build", "gf_ground_map_status", "gf_bedding_source_select", "gf_bedding_source_add",
    "gf_bedding_source_remove", "gf_bedding_clear", "gf_bedding_source_ids", "gf_layout_save", "gf_foundation_snapshot_select",
    "gf_analysis_load_mode", "gf_control_template_select", "gf_configured_load_select", "gf_manual_n_kn", "gf_manual_mx_knm",
    "gf_manual_my_knm", "gf_manual_k_kn_m3", "gf_manual_gap_m", "gf_manual_own_weight", "gf_foundation_source_reference",
    "gf_foundation_bedding_note", "gf_foundation_input_json", "gf_foundation_analyze", "gf_view_plan", "gf_view_section", "gf_view_3d",
    "gf_view_foundation_plan", "gf_view_foundation_3d", "gf_view_pressure", "gf_view_settlement",
    "gf_fit", "gf_show_unknown", "gf_show_terrain", "gf_show_contacts", "gf_visible_ige",
    "gf_show_boreholes", "gf_show_water", "gf_show_foundations", "gf_section_spacing",
    "gf_view_title", "gf_render_status", "gf_engineering_hold", "gf_canvas_host", "gf_canvas", "gf_scene_host", "gf_viewport",
    "gf_selection_label", "gf_inspector_text", "gf_report_text", "gf_query_kind", "gf_query_x", "gf_query_y",
    "gf_query_spacing", "gf_query_run", "gf_query_text",
];

#[derive(Clone, Debug)]
pub(super) struct GeoFoundationUi {
    pub active: bool,
    pub view: GeoFoundationView,
    pub active_tab: GeoFoundationTab,
    pub selected_model: Option<GroundModelId>,
    pub selected_scenario: Option<GroundScenarioId>,
    pub model_ids: Vec<GroundModelId>,
    pub selected_source: Option<SourceId>,
    pub selected_soil: Option<GeoSoilId>,
    pub selected_parameter_index: usize,
    pub selected_ige: Option<IgeId>,
    pub selected_borehole: Option<BoreholeId>,
    pub selected_triangle: Option<geo_core::TriangleId>,
    pub selected_contact: Option<geo_core::HorizonId>,
    pub selected_section: Option<GeoSectionId>,
    pub selected_water_observation: Option<geo_core::WaterObservationId>,
    pub water_editing_observation: bool,
    pub binding: Option<gf_api::GfBinding>,
    pub projection: Option<gf_api::GfProjection>,
    pub draft: Option<GeoFoundationDraft>,
    pub draft_dirty: bool,
    pub new_record: bool,
    pub binding_stale: bool,
    pub selected_snapshot: Option<geo_core::GeoSnapshotId>,
    pub snapshot_ids: Vec<geo_core::GeoSnapshotId>,
    pub layout_draft: Option<FoundationLayout>,
    pub layout_dirty: bool,
    pub layout_fields_dirty: bool,
    new_placement_ids: BTreeSet<FoundationId>,
    new_tier_ids: BTreeSet<String>,
    pub selected_placement: Option<FoundationId>,
    pub selected_foundation: Option<String>,
    pub placement_ids: Vec<FoundationId>,
    pub selected_tier_id: Option<String>,
    pub tier_ids: Vec<String>,
    pub layout_binding: Option<gf_api::GfBinding>,
    pub layout_link_model: Option<GroundModelId>,
    pub layout_link_scenario: Option<GroundScenarioId>,
    pub selected_foundation_snapshot: Option<ai_revit_core::EngineeringSnapshotId>,
    pub foundation_snapshot_ids: Vec<ai_revit_core::EngineeringSnapshotId>,
    pub selected_control_template: Option<FoundationId>,
    pub control_template_ids: Vec<FoundationId>,
    pub configured_load_source_ids: Vec<ai_revit_core::EngineeringSnapshotId>,
    pub selected_configured_load_source: Option<ai_revit_core::EngineeringSnapshotId>,
    pub analysis_uses_configured_reactions: bool,
    pub analysis_controls_dirty: bool,
    pub analysis_payload_dirty: bool,
    pub analysis_controls_binding: Option<gf_api::GfBinding>,
    pub analysis_controls_stale: bool,
    pub ground_map: Option<ai_revit_core::GeoFoundationGroundMap>,
    pub ground_map_key: Option<String>,
    pub ground_map_status: String,
    pub bedding_source_ids: Vec<SourceId>,
    pub bedding_source_option_ids: Vec<SourceId>,
    pub selected_bedding_source: Option<SourceId>,
    pub bedding_fields_dirty: bool,
    pub bedding_clear_requested: Option<FoundationId>,
    pub ground_link_model_ids: Vec<GroundModelId>,
    pub ground_link_scenario_ids: Vec<GroundScenarioId>,
    pub source_ids: Vec<SourceId>,
    pub soil_ids: Vec<GeoSoilId>,
    pub ige_ids: Vec<IgeId>,
    pub borehole_ids: Vec<BoreholeId>,
    pub triangle_ids: Vec<geo_core::TriangleId>,
    pub contact_ids: Vec<geo_core::HorizonId>,
    pub section_ids: Vec<GeoSectionId>,
    pub scenario_ids: Vec<GroundScenarioId>,
    pub water_observation_ids: Vec<geo_core::WaterObservationId>,
    pub interval_ids: Vec<geo_core::BoreholeIntervalId>,
    pub selected_interval: Option<geo_core::BoreholeIntervalId>,
    pub selected_contact_borehole: Option<BoreholeId>,
    pub unknown_zone_index: Option<usize>,
    pub triangle_vertex_ids: Vec<BoreholeId>,
    pub model_borehole_ids: Vec<BoreholeId>,
    pub query_text: String,
    query_result_binding: Option<String>,
    pub inspector_text: String,
    pub report_text: String,
    pub selected_record_key: Option<String>,
    pub selection: Option<gf_render::GeoPick>,
    selected_ground_sample_inspector: Option<(String, String)>,
    selected_layer_volume_inspector: Option<(String, String)>,
    pub scene_picks: BTreeMap<ObjectId, gf_render::GeoPick>,
    pub scene_sources: BTreeMap<ObjectId, gf_render::GeoObjectSources>,
    pub scene_source_fingerprint: Option<String>,
    pub scene_foundation_snapshot_fingerprint: Option<String>,
    camera: Camera2D,
    canvas_measured: bool,
    camera_change: Option<CanvasCameraChange>,
    pan_anchor: Option<DraftPoint>,
    screen: Arc<AnalysisCanvasLayer>,
    screen_key: Option<String>,
    scene_key: Option<String>,
    fit_pending: bool,
    pub show_unknown_zones: bool,
    pub show_terrain: bool,
    pub show_contacts: bool,
    pub show_boreholes: bool,
    pub show_water: bool,
    pub show_foundations: bool,
    pub visible_ige_filter: Option<IgeId>,
    pub section_max_spacing_m: f64,
    pub render_generation: u64,
    pub scene_generation: u64,
    pub status: String,
    pub attempt_status: Option<String>,
    pub attempt_message: String,
}

impl Default for GeoFoundationUi {
    fn default() -> Self {
        Self {
            active: false,
            view: GeoFoundationView::GroundPlan,
            active_tab: GeoFoundationTab::Soils,
            selected_model: None,
            selected_scenario: None,
            model_ids: Vec::new(),
            selected_source: None,
            selected_soil: None,
            selected_parameter_index: 0,
            selected_ige: None,
            selected_borehole: None,
            selected_triangle: None,
            selected_contact: None,
            selected_section: None,
            selected_water_observation: None,
            water_editing_observation: false,
            binding: None,
            projection: None,
            draft: None,
            draft_dirty: false,
            new_record: false,
            binding_stale: false,
            selected_snapshot: None,
            snapshot_ids: Vec::new(),
            layout_draft: None,
            layout_dirty: false,
            layout_fields_dirty: false,
            new_placement_ids: BTreeSet::new(),
            new_tier_ids: BTreeSet::new(),
            selected_placement: None,
            selected_foundation: None,
            placement_ids: Vec::new(),
            selected_tier_id: None,
            tier_ids: Vec::new(),
            layout_binding: None,
            layout_link_model: None,
            layout_link_scenario: None,
            selected_foundation_snapshot: None,
            foundation_snapshot_ids: Vec::new(),
            selected_control_template: None,
            control_template_ids: Vec::new(),
            configured_load_source_ids: Vec::new(),
            selected_configured_load_source: None,
            analysis_uses_configured_reactions: false,
            analysis_controls_dirty: false,
            analysis_payload_dirty: false,
            analysis_controls_binding: None,
            analysis_controls_stale: false,
            ground_map: None,
            ground_map_key: None,
            ground_map_status: "No midpoint map requested. Sampling is not continuous coverage.".into(),
            bedding_source_ids: Vec::new(),
            bedding_source_option_ids: Vec::new(),
            selected_bedding_source: None,
            bedding_fields_dirty: false,
            bedding_clear_requested: None,
            ground_link_model_ids: Vec::new(),
            ground_link_scenario_ids: Vec::new(),
            source_ids: Vec::new(),
            soil_ids: Vec::new(),
            ige_ids: Vec::new(),
            borehole_ids: Vec::new(),
            triangle_ids: Vec::new(),
            contact_ids: Vec::new(),
            section_ids: Vec::new(),
            scenario_ids: Vec::new(),
            water_observation_ids: Vec::new(),
            interval_ids: Vec::new(),
            selected_interval: None,
            selected_contact_borehole: None,
            unknown_zone_index: None,
            triangle_vertex_ids: Vec::new(),
            model_borehole_ids: Vec::new(),
            query_text: "No query result for the current selected snapshot.".into(),
            query_result_binding: None,
            inspector_text: "No Geo record selected".into(),
            report_text: "No current Geo/Foundation projection.".into(),
            selected_record_key: None,
            selection: None,
            selected_ground_sample_inspector: None,
            selected_layer_volume_inspector: None,
            scene_picks: BTreeMap::new(),
            scene_sources: BTreeMap::new(),
            scene_source_fingerprint: None,
            scene_foundation_snapshot_fingerprint: None,
            camera: Camera2D::default(),
            canvas_measured: false,
            camera_change: None,
            pan_anchor: None,
            screen: Arc::new(AnalysisCanvasLayer::default()),
            screen_key: None,
            scene_key: None,
            fit_pending: false,
            show_unknown_zones: true,
            show_terrain: true,
            show_contacts: true,
            show_boreholes: true,
            show_water: true,
            show_foundations: true,
            visible_ige_filter: None,
            section_max_spacing_m: 1.0,
            render_generation: 0,
            scene_generation: 0,
            status: "MODEL_INCOMPLETE · no current Document records".into(),
            attempt_status: None,
            attempt_message: String::new(),
        }
    }
}

impl GeoFoundationUi {
    fn draft_id<'a>(prefix: &str, existing: impl Iterator<Item = &'a str>) -> String {
        let used: BTreeSet<&str> = existing.collect();
        for index in 1..=4096 {
            let candidate = format!("draft-{prefix}-{index}");
            if !used.contains(candidate.as_str()) {
                return candidate;
            }
        }
        format!("draft-{prefix}-pending")
    }

    pub(super) fn set_attempt(&mut self, code: Option<&str>, message: impl Into<String>) {
        self.attempt_status = code.map(str::to_owned);
        self.attempt_message = message.into();
    }

    pub(super) fn mark_draft_dirty(&mut self) {
        self.draft_dirty = true;
        self.ground_map = None;
        self.ground_map_key = None;
        self.ground_map_status = "GEO record draft changed · previous midpoint map cleared.".into();
        self.render_generation = self.render_generation.saturating_add(1);
        self.scene_generation = self.scene_generation.saturating_add(1);
    }

    pub(super) fn clear_draft(&mut self) {
        self.draft = None;
        self.draft_dirty = false;
        self.binding_stale = false;
    }

    pub(super) fn clear_current_results(&mut self) {
        self.query_text = "No query result for the current selected snapshot.".into();
        self.query_result_binding = None;
        self.render_generation = self.render_generation.saturating_add(1);
        self.scene_generation = self.scene_generation.saturating_add(1);
    }
}

fn selector_index<T: PartialEq>(items: &[T], selected: Option<&T>) -> usize {
    selected
        .and_then(|selected| items.iter().position(|item| item == selected))
        .unwrap_or(0)
}

fn selector_labels_or_empty(labels: Vec<String>, empty_label: &str) -> Vec<String> {
    if labels.is_empty() {
        vec![empty_label.to_owned()]
    } else {
        labels
    }
}

fn parameter_editor_fields<T: serde::Serialize>(parameter: &Parameter<T>) -> (usize, String, String, String, String) {
    match parameter {
        Parameter::Missing { reason } => (0, String::new(), reason.clone(), String::new(), String::new()),
        Parameter::Known { value, source_ids, .. } => (
            1,
            serde_json::to_string(value).unwrap_or_default(),
            String::new(),
            source_ids_text(source_ids),
            String::new(),
        ),
        Parameter::Conflicting { candidates } => (
            2,
            String::new(),
            String::new(),
            String::new(),
            serde_json::to_string_pretty(candidates).unwrap_or_else(|_| "[]".into()),
        ),
    }
}

fn foundation_placement_id(placement: &FoundationPlacement) -> &FoundationId {
    match placement {
        FoundationPlacement::Raft { id, .. }
        | FoundationPlacement::Strip { id, .. }
        | FoundationPlacement::Isolated { id, .. }
        | FoundationPlacement::Stepped { id, .. } => id,
    }
}

fn foundation_placement_kind(placement: &FoundationPlacement) -> &'static str {
    match placement {
        FoundationPlacement::Raft { .. } => "Raft",
        FoundationPlacement::Strip { .. } => "Strip",
        FoundationPlacement::Isolated { .. } => "Isolated",
        FoundationPlacement::Stepped { .. } => "Stepped",
    }
}

fn soil_parameter_at(parameters: &SoilParameters, index: usize) -> &Parameter<f64> {
    match index {
        0 => &parameters.unit_weight_kn_m3,
        1 => &parameters.cohesion_kpa,
        2 => &parameters.friction_angle_deg,
        3 => &parameters.elastic_modulus_kpa,
        _ => &parameters.poisson_ratio,
    }
}

fn parameter_from_fields(
    state: usize,
    value_text: &str,
    reason_text: &str,
    source_text: &str,
    candidates_text: &str,
    unit: ParameterUnit,
    previous: Option<&Parameter<f64>>,
) -> Result<Parameter<f64>, String> {
    match state {
        0 => {
            let reason = reason_text.trim();
            if reason.is_empty() {
                return Err("Missing requires an explicit reason; no zero/default is substituted".into());
            }
            Ok(Parameter::Missing { reason: reason.to_owned() })
        }
        1 => {
            let value = parse_finite_field(value_text, "parameter value")?;
            let source_ids = parse_source_ids(source_text)?;
            if source_ids.is_empty() {
                return Err("Known values require one or more source IDs".into());
            }
            let mut metadata = match previous {
                Some(Parameter::Known { metadata, .. }) => metadata.clone(),
                _ => ParameterMetadata::default(),
            };
            metadata.unit = unit;
            metadata.confidence_class = ConfidenceClass::Unverified;
            metadata.engineering_verified = false;
            Ok(Parameter::Known { value, source_ids, metadata })
        }
        2 => {
            let candidates: Vec<ParameterCandidate<f64>> = serde_json::from_str(candidates_text.trim())
                .map_err(|error| format!("Conflicting candidates JSON: {error}"))?;
            if candidates.is_empty() {
                return Err("Conflicting requires at least one explicit candidate".into());
            }
            Ok(Parameter::Conflicting { candidates })
        }
        _ => Err("Unknown parameter state".into()),
    }
}

fn replace_soil_parameter(parameters: &mut SoilParameters, index: usize, value: Parameter<f64>) {
    match index {
        0 => parameters.unit_weight_kn_m3 = value,
        1 => parameters.cohesion_kpa = value,
        2 => parameters.friction_angle_deg = value,
        3 => parameters.elastic_modulus_kpa = value,
        _ => parameters.poisson_ratio = value,
    }
}

impl App {
    pub(super) fn geo_foundation_apply_visibility(&self, cx: &mut Cx) {
        let active = self.geo_foundation_ui.active;
        self.ui.widget(cx, ids!(gf_workspace)).set_visible(cx, active);
        self.ui.button(cx, ids!(workspace_geo_foundation)).set_text(
            cx,
            if active { "● GEO / Foundation" } else { "GEO / Foundation" },
        );
        if !active {
            return;
        }
        let record_editor = self.document.is_some()
            && !self.geo_foundation_ui.binding_stale
            && (self.geo_foundation_ui.draft.is_some()
                || self.geo_foundation_ui.new_record
                || self.geo_foundation_ui.active_tab == GeoFoundationTab::Contacts && self.geo_foundation_ui.selected_contact.is_some()
                || self.geo_foundation_ui.active_tab == GeoFoundationTab::UnknownZones && self.geo_foundation_ui.unknown_zone_index.is_some());
        let layout_editor = self.document.is_some() && !self.geo_foundation_ui.binding_stale;
        let numerical_editor = layout_editor && !self.geo_foundation_ui.analysis_controls_stale;
        for widget in [
            ids!(gf_source_id), ids!(gf_source_title), ids!(gf_source_reference), ids!(gf_source_sha256), ids!(gf_source_observed_at),
            ids!(gf_soil_id), ids!(gf_soil_name), ids!(gf_parameter_value), ids!(gf_parameter_missing_reason), ids!(gf_parameter_source_ids), ids!(gf_parameter_candidates),
            ids!(gf_ige_id), ids!(gf_ige_name), ids!(gf_ige_source_ids), ids!(gf_borehole_id), ids!(gf_collar_x), ids!(gf_collar_y), ids!(gf_collar_z),
            ids!(gf_borehole_collar_vertical_datum), ids!(gf_borehole_source_ids), ids!(gf_borehole_total_depth), ids!(gf_borehole_total_depth_reason), ids!(gf_borehole_total_depth_sources),
            ids!(gf_borehole_date), ids!(gf_borehole_crs), ids!(gf_borehole_depth_datum), ids!(gf_borehole_survey_sources), ids!(gf_borehole_evidence_sources),
            ids!(gf_interval_id), ids!(gf_interval_top), ids!(gf_interval_bottom), ids!(gf_interval_source_ids), ids!(gf_ground_model_id), ids!(gf_ground_model_name),
            ids!(gf_frame_name), ids!(gf_frame_crs), ids!(gf_frame_datum), ids!(gf_model_source_ids), ids!(gf_triangle_id), ids!(gf_triangle_source_ids),
            ids!(gf_contact_id), ids!(gf_contact_vertical_datum), ids!(gf_contact_elevation), ids!(gf_contact_missing_reason), ids!(gf_contact_source_ids), ids!(gf_contact_candidates),
            ids!(gf_unknown_polygon), ids!(gf_unknown_reason), ids!(gf_unknown_source_ids), ids!(gf_scenario_id), ids!(gf_scenario_name), ids!(gf_scenario_source_ids),
            ids!(gf_water_source_ids), ids!(gf_water_observation_ids_text), ids!(gf_water_observation_id), ids!(gf_water_x), ids!(gf_water_y), ids!(gf_water_observed_at),
            ids!(gf_water_elevation), ids!(gf_water_elevation_source_ids), ids!(gf_section_id), ids!(gf_section_alignment), ids!(gf_section_source_ids),
        ] {
            self.ui.text_input(cx, widget).set_is_read_only(cx, !record_editor);
        }
        for widget in [ids!(gf_layout_id),ids!(gf_placement_id),ids!(gf_placement_x),ids!(gf_placement_y),ids!(gf_placement_z),ids!(gf_placement_length_x),ids!(gf_placement_width_y),ids!(gf_placement_thickness),ids!(gf_placement_strip_length),ids!(gf_placement_strip_width),ids!(gf_placement_rotation),ids!(gf_tier_id),ids!(gf_tier_x),ids!(gf_tier_y),ids!(gf_tier_z),ids!(gf_tier_length_x),ids!(gf_tier_width_y),ids!(gf_tier_thickness)] {
            self.ui.text_input(cx, widget).set_is_read_only(cx, !layout_editor);
        }
        for widget in [ids!(gf_manual_n_kn), ids!(gf_manual_mx_knm), ids!(gf_manual_my_knm), ids!(gf_foundation_source_reference), ids!(gf_foundation_input_json)] {
            self.ui.text_input(cx, widget).set_is_read_only(cx, !numerical_editor || self.geo_foundation_ui.analysis_uses_configured_reactions);
        }
        for widget in [ids!(gf_manual_k_kn_m3), ids!(gf_manual_gap_m), ids!(gf_foundation_bedding_note)] {
            self.ui.text_input(cx, widget).set_is_read_only(cx, !numerical_editor);
        }
        self.ui.button(cx, ids!(gf_apply)).set_enabled(cx, self.document.is_some() && !self.geo_foundation_ui.binding_stale && self.geo_foundation_ui.draft_dirty);
        self.ui.button(cx, ids!(gf_layout_save)).set_enabled(cx, self.document.is_some() && !self.geo_foundation_ui.binding_stale && self.geo_foundation_ui.layout_dirty);
        self.ui.button(cx, ids!(gf_save_snapshot)).set_enabled(cx, self.document.is_some() && !self.geo_foundation_ui.binding_stale && !self.geo_foundation_ui.draft_dirty && !self.geo_foundation_ui.layout_dirty);
        let has_numerical_basis = self.geo_foundation_ui.selected_control_template.is_some()
            || (!self.geo_foundation_ui.analysis_uses_configured_reactions && !self.ui.text_input(cx, ids!(gf_foundation_input_json)).text().trim().is_empty());
        let has_load_source = !self.geo_foundation_ui.analysis_uses_configured_reactions || self.geo_foundation_ui.selected_configured_load_source.is_some();
        let analysis_enabled = self.document.is_some() && !self.geo_foundation_ui.binding_stale && !self.geo_foundation_ui.analysis_controls_stale
            && !self.geo_foundation_ui.draft_dirty && !self.geo_foundation_ui.layout_dirty && !self.geo_foundation_ui.layout_fields_dirty
            && has_numerical_basis && has_load_source
            && self.geo_foundation_ui.projection.as_ref().is_some_and(|projection| projection.layout.as_ref().is_some_and(|layout| self.geo_foundation_ui.selected_placement.as_ref().is_some_and(|id| layout.input.layout.elements.iter().any(|element| foundation_placement_id(element) == id && matches!(element, FoundationPlacement::Raft { .. })))));
        self.ui.button(cx, ids!(gf_foundation_analyze)).set_enabled(cx, analysis_enabled);
        self.ui.button(cx, ids!(gf_query_run)).set_enabled(cx, self.document.is_some() && self.geo_foundation_ui.projection.as_ref().is_some_and(|projection| projection.resolved.is_some()));
        let plan = matches!(
            self.geo_foundation_ui.view,
            GeoFoundationView::GroundPlan
                | GeoFoundationView::Section
                | GeoFoundationView::FoundationPlan
                | GeoFoundationView::Pressure2D
                | GeoFoundationView::Settlement2D
        );
        self.ui.widget(cx, ids!(gf_canvas_host)).set_visible(cx, plan);
        self.ui.widget(cx, ids!(gf_scene_host)).set_visible(cx, !plan);
        for (id, visible) in [
            (ids!(history_panel), false),
            (ids!(inspector_panel), false),
            (ids!(viewport_panel), false),
            (ids!(geotech_parameters), false),
            (ids!(geotech_results), false),
            (ids!(geotech_actions), false),
            (ids!(circular_parameters_host), false),
            (ids!(circular_results), false),
            (ids!(circular_actions), false),
            (ids!(circular_view_status), false),
            (ids!(model_toolbar), false),
            (ids!(drafting_toolbar), false),
            (ids!(drafting_command_panel), false),
            (ids!(viewport_3d_host), false),
            (ids!(drafting_2d_host), false),
        ] {
            self.ui.widget(cx, id).set_visible(cx, visible);
        }
        self.geo_foundation_set_tab_host(cx);
    }

    pub(super) fn geo_foundation_set_active(&mut self, cx: &mut Cx, active: bool) {
        if active {
            let _ = self.set_workspace(cx, "3d");
        }
        self.geo_foundation_ui.active = active;
        self.geo_foundation_ui.pan_anchor = None;
        cx.revert_key_focus();
        if active {
            self.geo_foundation_apply_visibility(cx);
            self.geo_foundation_refresh_projection(cx);
            self.geo_foundation_render_changed(cx, true);
        } else {
            self.refresh_workspace_visibility(cx);
            self.refresh_drafting(cx, true);
            self.geo_foundation_apply_visibility(cx);
        }
    }

    pub(super) fn geo_foundation_after_event(&mut self, cx: &mut Cx, event: &Event) {
        if !self.geo_foundation_ui.active {
            return;
        }
        if self.foundation_structural_ui.active || self.structural_configurator_ui.active
            || self.geotech_ui.active || self.circular_ui.panel.active
        {
            self.geo_foundation_set_active(cx, false);
            return;
        }
        if self.document.is_none() {
            self.geo_foundation_refresh_projection(cx);
            self.geo_foundation_apply_visibility(cx);
            return;
        }
        if matches!(event, Event::Draw(_) | Event::Actions(_)) {
            self.geo_foundation_refresh_projection(cx);
            self.geo_foundation_render_changed(cx, false);
            self.geo_foundation_apply_visibility(cx);
        }
    }

    pub(super) fn handle_geo_foundation_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(workspace_geo_foundation)).clicked(actions) {
            self.geo_foundation_set_active(cx, !self.geo_foundation_ui.active);
            return;
        }
        if self.geo_foundation_ui.active {
            let switched = [
                id!(workspace_3d), id!(workspace_2d), id!(workspace_engineering),
                id!(workspace_circular), id!(workspace_foundation_structural),
                id!(workspace_structural_configurator),
            ].into_iter().any(|id| self.ui.button(cx, &[id]).clicked(actions));
            if switched {
                self.geo_foundation_set_active(cx, false);
                return;
            }
        }
        if !self.geo_foundation_ui.active {
            return;
        }

        if self.ui.button(cx, ids!(gf_reload)).clicked(actions) {
            self.geo_foundation_reload(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_load_demo)).clicked(actions) {
            self.geo_foundation_load_demo_ui(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_apply)).clicked(actions) {
            self.geo_foundation_apply_record(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_save_snapshot)).clicked(actions) {
            self.geo_foundation_save_snapshot(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_layout_save)).clicked(actions) {
            self.geo_foundation_save_layout(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_foundation_analyze)).clicked(actions) {
            self.geo_foundation_run_foundation_analysis(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_ground_map_build)).clicked(actions) {
            self.geo_foundation_build_ground_map(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_query_run)).clicked(actions) {
            self.geo_foundation_run_query(cx);
            return;
        }

        self.geo_foundation_handle_analysis_controls(cx, actions);
        self.geo_foundation_handle_selection_actions(cx, actions);
        self.geo_foundation_handle_record_creation(cx, actions);
        self.geo_foundation_handle_layout_actions(cx, actions);
        self.geo_foundation_handle_view_actions(cx, actions);
        self.geo_foundation_handle_input_changes(cx, actions);

        let canvas_actions = self.ui.drafting_canvas(cx, ids!(gf_canvas)).actions(actions);
        for action in canvas_actions {
            self.geo_foundation_canvas_action(cx, action);
        }
        let viewport_actions = self.ui.cad_viewport(cx, ids!(gf_viewport)).actions(actions);
        let mut selection_changed = false;
        let mut picked = None;
        for action in viewport_actions {
            match action {
                crate::viewport::CadViewportAction::Viewer(
                    ViewerEvent::ObjectPicked { .. } | ViewerEvent::SelectionChanged { .. },
                ) => selection_changed = true,
                crate::viewport::CadViewportAction::Inspect(object_id) => {
                    picked = Some(object_id);
                    selection_changed = true;
                }
                _ => {}
            }
        }
        if selection_changed {
            if picked.is_none() {
                picked = self.ui.cad_viewport(cx, ids!(gf_viewport)).borrow()
                    .and_then(|viewport| viewport.viewer().scene().selected_ids().first().copied());
            }
            let semantic = picked.and_then(|object_id| self.geo_foundation_ui.scene_picks.get(&object_id).cloned());
            self.geo_foundation_select_pick(cx, semantic);
            self.geo_foundation_render_changed(cx, false);
        }
    }

    fn geo_foundation_handle_selection_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let dirty = self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty;
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_snapshot_select)).selected(actions) {
            if dirty {
                self.geo_foundation_restore_dropdown(cx, ids!(gf_snapshot_select), self.geo_foundation_ui.selected_snapshot.as_ref()
                    .and_then(|id| self.geo_foundation_ui.snapshot_ids.iter().position(|value| value == id)).map_or(0, |index| index + 1));
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload the current draft before changing the selected GEO closure; draft text was kept.");
            } else {
                self.geo_foundation_ui.selected_snapshot = if index == 0 { None } else { self.geo_foundation_ui.snapshot_ids.get(index - 1).cloned() };
                self.geo_foundation_ui.selection = None;
                self.geo_foundation_ui.selected_foundation_snapshot = None;
                self.geo_foundation_refresh_projection(cx);
                self.geo_foundation_render_changed(cx, false);
            }
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_model_select)).selected(actions) {
            if dirty { self.geo_foundation_restore_dropdown(cx, ids!(gf_model_select), selector_index(&self.geo_foundation_ui.model_ids, self.geo_foundation_ui.selected_model.as_ref())); self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload before changing the model. The draft is retained."); }
            else {
                self.geo_foundation_ui.selected_model = self.geo_foundation_ui.model_ids.get(index).cloned();
                self.geo_foundation_ui.selected_scenario = None;
                self.geo_foundation_ui.selected_snapshot = None;
                self.geo_foundation_refresh_projection(cx);
                self.geo_foundation_render_changed(cx, false);
            }
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_scenario_select)).selected(actions) {
            if dirty { self.geo_foundation_restore_dropdown(cx, ids!(gf_scenario_select), self.geo_foundation_ui.selected_scenario.as_ref().and_then(|id| self.geo_foundation_ui.scenario_ids.iter().position(|value| value == id)).map_or(0, |index| index + 1)); self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload before changing the water scenario. The draft is retained."); }
            else {
                self.geo_foundation_ui.selected_scenario = if index == 0 { None } else { self.geo_foundation_ui.scenario_ids.get(index - 1).cloned() };
                self.geo_foundation_ui.selected_snapshot = None;
                self.geo_foundation_refresh_projection(cx);
                self.geo_foundation_render_changed(cx, false);
            }
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_source_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_source_select), selector_index(&self.geo_foundation_ui.source_ids, self.geo_foundation_ui.selected_source.as_ref())) { return; }
            self.geo_foundation_ui.selected_source = self.geo_foundation_ui.source_ids.get(index).cloned();
            self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_soil_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_soil_select), selector_index(&self.geo_foundation_ui.soil_ids, self.geo_foundation_ui.selected_soil.as_ref())) { return; }
            self.geo_foundation_ui.selected_soil = self.geo_foundation_ui.soil_ids.get(index).cloned();
            self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_ige_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_ige_select), selector_index(&self.geo_foundation_ui.ige_ids, self.geo_foundation_ui.selected_ige.as_ref())) { return; }
            self.geo_foundation_ui.selected_ige = self.geo_foundation_ui.ige_ids.get(index).cloned();
            self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_borehole_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_borehole_select), selector_index(&self.geo_foundation_ui.borehole_ids, self.geo_foundation_ui.selected_borehole.as_ref())) { return; }
            self.geo_foundation_ui.selected_borehole = self.geo_foundation_ui.borehole_ids.get(index).cloned();
            self.geo_foundation_ui.selected_interval = None;
            self.geo_foundation_sync_selectors(cx); self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_interval_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_interval_select), selector_index(&self.geo_foundation_ui.interval_ids, self.geo_foundation_ui.selected_interval.as_ref())) { return; }
            self.geo_foundation_ui.selected_interval = self.geo_foundation_ui.interval_ids.get(index).cloned();
            self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_triangle_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_triangle_select), selector_index(&self.geo_foundation_ui.triangle_ids, self.geo_foundation_ui.selected_triangle.as_ref())) { return; }
            self.geo_foundation_ui.selected_triangle = self.geo_foundation_ui.triangle_ids.get(index).cloned();
            self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_contact_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_contact_select), selector_index(&self.geo_foundation_ui.contact_ids, self.geo_foundation_ui.selected_contact.as_ref())) { return; }
            self.geo_foundation_ui.selected_contact = self.geo_foundation_ui.contact_ids.get(index).cloned();
            self.geo_foundation_ui.selected_contact_borehole = None; self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_unknown_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_unknown_select), self.geo_foundation_ui.unknown_zone_index.unwrap_or(0)) { return; }
            self.geo_foundation_ui.unknown_zone_index = Some(index); self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_water_observation_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_water_observation_select), selector_index(&self.geo_foundation_ui.water_observation_ids, self.geo_foundation_ui.selected_water_observation.as_ref())) { return; }
            self.geo_foundation_ui.water_editing_observation = true;
            self.geo_foundation_ui.selected_water_observation = self.geo_foundation_ui.water_observation_ids.get(index).cloned();
            self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_section_select)).selected(actions) {
            if !self.geo_foundation_guard_dirty_selection(cx, ids!(gf_section_select), selector_index(&self.geo_foundation_ui.section_ids, self.geo_foundation_ui.selected_section.as_ref())) { return; }
            self.geo_foundation_ui.selected_section = self.geo_foundation_ui.section_ids.get(index).cloned(); self.geo_foundation_load_selected_editor(cx); self.geo_foundation_update_record_inspector(cx); self.geo_foundation_render_changed(cx, false); return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_foundation_snapshot_select)).selected(actions) {
            if self.geo_foundation_ui.layout_dirty || self.geo_foundation_ui.binding_stale {
                let current = self.geo_foundation_ui.selected_foundation_snapshot.as_ref().and_then(|id| self.geo_foundation_ui.foundation_snapshot_ids.iter().position(|value| value == id)).map_or(0, |index| index + 1);
                self.geo_foundation_restore_dropdown(cx, ids!(gf_foundation_snapshot_select), current);
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload the layout draft before selecting another analysis snapshot.");
            } else {
                self.geo_foundation_ui.selected_foundation_snapshot = if index == 0 { None } else { self.geo_foundation_ui.foundation_snapshot_ids.get(index - 1).copied() };
                self.geo_foundation_render_changed(cx, false); self.geo_foundation_update_record_inspector(cx);
            }
        }
    }

    fn geo_foundation_restore_dropdown(&mut self, cx: &mut Cx, widget: &[LiveId], index: usize) {
        self.ui.drop_down(cx, widget).set_selected_item(cx, index);
    }

    fn geo_foundation_handle_analysis_controls(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(gf_bedding_source_add)).clicked(actions) {
            self.geo_foundation_add_bedding_source(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_bedding_source_remove)).clicked(actions) {
            self.geo_foundation_remove_bedding_source(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_bedding_clear)).clicked(actions) {
            self.geo_foundation_clear_bedding_declaration(cx);
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_bedding_source_select)).selected(actions) {
            self.geo_foundation_ui.selected_bedding_source = index.checked_sub(1).and_then(|index| self.geo_foundation_ui.bedding_source_option_ids.get(index).cloned());
            self.ui.drop_down(cx, ids!(gf_bedding_source_select)).set_selected_item(cx, self.geo_foundation_ui.selected_bedding_source.as_ref()
                .map_or(0, |id| selector_index(&self.geo_foundation_ui.bedding_source_option_ids, Some(id)) + 1));
            return;
        }
        let mut analysis_payload_changed = false;
        let mut bedding_changed = false;
        for widget in [ids!(gf_manual_k_kn_m3), ids!(gf_foundation_bedding_note)] {
            bedding_changed |= self.ui.text_input(cx, widget).changed(actions).is_some();
        }
        for widget in [
            ids!(gf_manual_n_kn), ids!(gf_manual_mx_knm), ids!(gf_manual_my_knm), ids!(gf_manual_gap_m),
            ids!(gf_foundation_source_reference), ids!(gf_foundation_input_json),
        ] { analysis_payload_changed |= self.ui.text_input(cx, widget).changed(actions).is_some(); }
        analysis_payload_changed |= self.ui.check_box(cx, ids!(gf_manual_own_weight)).changed(actions).is_some();
        if analysis_payload_changed || bedding_changed {
            if analysis_payload_changed { self.geo_foundation_ui.analysis_payload_dirty = true; }
            self.geo_foundation_mark_analysis_controls_dirty(cx);
            if bedding_changed {
                self.geo_foundation_ui.bedding_fields_dirty = true;
                self.geo_foundation_ui.bedding_clear_requested = None;
                self.geo_foundation_mark_layout_dirty(cx);
            }
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_analysis_load_mode)).selected(actions) {
            self.geo_foundation_ui.analysis_uses_configured_reactions = index == 1;
            self.ui.drop_down(cx, ids!(gf_analysis_load_mode)).set_selected_item(cx, usize::from(self.geo_foundation_ui.analysis_uses_configured_reactions));
            self.geo_foundation_mark_analysis_controls_dirty(cx);
            self.geo_foundation_apply_visibility(cx);
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_control_template_select)).selected(actions) {
            if self.geo_foundation_ui.analysis_controls_dirty {
                let current = self.geo_foundation_ui.selected_control_template.as_ref()
                    .map_or(0, |id| selector_index(&self.geo_foundation_ui.control_template_ids, Some(id)) + 1);
                self.geo_foundation_restore_dropdown(cx, ids!(gf_control_template_select), current);
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Run Analyze or reload before changing the numerical control template; entered values are retained.");
                return;
            }
            let selected = index.checked_sub(1).and_then(|index| self.geo_foundation_ui.control_template_ids.get(index).cloned());
            self.geo_foundation_ui.selected_control_template = selected.clone();
            self.ui.drop_down(cx, ids!(gf_control_template_select)).set_selected_item(cx, selected.as_ref()
                .map_or(0, |id| selector_index(&self.geo_foundation_ui.control_template_ids, Some(id)) + 1));
            if let Some(template_id) = selected {
                let template = self.geo_foundation_ui.projection.as_ref().and_then(|projection| projection.control_templates.iter()
                    .find(|(id, _)| id == &template_id).map(|(_, input)| input.clone()));
                if let Some(template) = template {
                    self.geo_foundation_fill_analysis_template(cx, &template);
                    self.geo_foundation_ui.analysis_controls_binding = self.geo_foundation_ui.projection.as_ref().map(|projection| projection.binding.clone());
                    self.geo_foundation_mark_analysis_controls_dirty(cx);
                } else {
                    self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "The selected numerical control template is no longer in this Document projection.");
                }
            } else {
                self.geo_foundation_mark_analysis_controls_dirty(cx);
            }
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_configured_load_select)).selected(actions) {
            self.geo_foundation_ui.selected_configured_load_source = index.checked_sub(1)
                .and_then(|index| self.geo_foundation_ui.configured_load_source_ids.get(index).copied());
            self.ui.drop_down(cx, ids!(gf_configured_load_select)).set_selected_item(cx, self.geo_foundation_ui.selected_configured_load_source.as_ref()
                .map_or(0, |id| selector_index(&self.geo_foundation_ui.configured_load_source_ids, Some(id)) + 1));
            self.geo_foundation_mark_analysis_controls_dirty(cx);
            return;
        }
    }

    fn geo_foundation_mark_analysis_controls_dirty(&mut self, cx: &mut Cx) {
        self.geo_foundation_ui.analysis_controls_dirty = true;
        if self.geo_foundation_ui.analysis_controls_binding.is_none() {
            self.geo_foundation_ui.analysis_controls_binding = self.geo_foundation_ui.projection.as_ref().map(|projection| projection.binding.clone());
        }
        self.geo_foundation_ui.analysis_controls_stale = self.geo_foundation_ui.projection.as_ref()
            .is_none_or(|projection| self.geo_foundation_ui.analysis_controls_binding.as_ref() != Some(&projection.binding));
        self.geo_foundation_ui.selected_foundation_snapshot = None;
        self.geo_foundation_ui.scene_key = None;
        self.geo_foundation_clear_scene(cx);
        self.geo_foundation_show_status(cx, if self.geo_foundation_ui.analysis_controls_stale { "STALE_BINDING" } else { "INPUT_ONLY" },
            "Unapplied numerical-control draft · no previous pressure or settlement snapshot is presented as the draft result.");
        self.geo_foundation_apply_visibility(cx);
    }

    fn geo_foundation_fill_analysis_template(&mut self, cx: &mut Cx, input: &RigidMatInput) {
        let json = serde_json::to_string_pretty(input).unwrap_or_default();
        self.ui.text_input(cx, ids!(gf_foundation_input_json)).set_text(cx, &json);
        self.ui.text_input(cx, ids!(gf_manual_n_kn)).set_text(cx, &input.load.n_kn.to_string());
        self.ui.text_input(cx, ids!(gf_manual_mx_knm)).set_text(cx, &(-input.load.mx_pressure_knm).to_string());
        self.ui.text_input(cx, ids!(gf_manual_my_knm)).set_text(cx, &input.load.my_pressure_knm.to_string());
        match &input.soil.support {
            foundation_core::SupportField::Uniform { k_kn_m3, gap_m } => {
                self.ui.text_input(cx, ids!(gf_manual_k_kn_m3)).set_text(cx, &k_kn_m3.to_string());
                self.ui.text_input(cx, ids!(gf_manual_gap_m)).set_text(cx, &gap_m.to_string());
            }
            foundation_core::SupportField::PerCell { .. } => {
                self.ui.text_input(cx, ids!(gf_manual_k_kn_m3)).set_text(cx, "");
                self.ui.text_input(cx, ids!(gf_manual_gap_m)).set_text(cx, "");
            }
        }
        self.ui.text_input(cx, ids!(gf_foundation_source_reference)).set_text(cx, "");
        self.ui.text_input(cx, ids!(gf_foundation_bedding_note)).set_text(cx, &input.soil.assumption_note);
        self.ui.check_box(cx, ids!(gf_manual_own_weight)).set_active(cx,
            matches!(input.self_weight_policy, foundation_core::SelfWeightPolicy::IncludeOnce { .. }), Animate::No);
        let selected_placement = self.geo_foundation_ui.selected_placement.clone();
        self.geo_foundation_load_bedding_fields(cx, selected_placement.as_ref());
    }

    fn geo_foundation_clear_analysis_controls(&mut self, cx: &mut Cx) {
        for widget in [
            ids!(gf_manual_n_kn), ids!(gf_manual_mx_knm), ids!(gf_manual_my_knm),
            ids!(gf_manual_k_kn_m3), ids!(gf_manual_gap_m), ids!(gf_foundation_source_reference),
            ids!(gf_foundation_bedding_note), ids!(gf_foundation_input_json),
        ] { self.geo_foundation_set_input(cx, widget, ""); }
        self.geo_foundation_ui.selected_control_template = None;
        self.geo_foundation_ui.selected_configured_load_source = None;
        self.geo_foundation_ui.analysis_uses_configured_reactions = false;
        self.geo_foundation_ui.analysis_controls_dirty = false;
        self.geo_foundation_ui.analysis_payload_dirty = false;
        self.geo_foundation_ui.analysis_controls_binding = None;
        self.geo_foundation_ui.analysis_controls_stale = false;
        self.geo_foundation_ui.bedding_source_ids.clear();
        self.geo_foundation_ui.selected_bedding_source = None;
        self.geo_foundation_ui.bedding_fields_dirty = false;
        self.geo_foundation_ui.bedding_clear_requested = None;
        self.ui.text_input(cx, ids!(gf_bedding_source_ids)).set_text(cx, "No authored bedding sources selected");
        self.ui.drop_down(cx, ids!(gf_bedding_source_select)).set_selected_item(cx, 0);
        self.ui.check_box(cx, ids!(gf_manual_own_weight)).set_active(cx, false, Animate::No);
        self.ui.drop_down(cx, ids!(gf_analysis_load_mode)).set_selected_item(cx, 0);
        self.ui.drop_down(cx, ids!(gf_control_template_select)).set_selected_item(cx, 0);
        self.ui.drop_down(cx, ids!(gf_configured_load_select)).set_selected_item(cx, 0);
    }

    fn geo_foundation_bedding_sources_changed(&mut self, cx: &mut Cx) {
        let text = if self.geo_foundation_ui.bedding_source_ids.is_empty() {
            "No authored bedding sources selected".to_owned()
        } else {
            self.geo_foundation_ui.bedding_source_ids.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
        };
        self.ui.text_input(cx, ids!(gf_bedding_source_ids)).set_text(cx, &text);
        self.geo_foundation_ui.bedding_fields_dirty = true;
        self.geo_foundation_ui.bedding_clear_requested = None;
        self.geo_foundation_mark_analysis_controls_dirty(cx);
        self.geo_foundation_mark_layout_dirty(cx);
    }

    fn geo_foundation_add_bedding_source(&mut self, cx: &mut Cx) {
        let Some(source_id) = self.geo_foundation_ui.selected_bedding_source.clone() else {
            self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Choose a source ID from the current GEO registry before adding it to the bedding declaration.");
            return;
        };
        let Some(projection) = self.geo_foundation_ui.projection.as_ref() else {
            self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Reload a current GEO projection before selecting a bedding source.");
            return;
        };
        if !projection.state.records.sources.contains_key(&source_id)
            || !self.geo_foundation_ui.bedding_source_option_ids.contains(&source_id)
            || !projection.selected_snapshot.as_ref().is_some_and(|snapshot| snapshot.records.sources.contains_key(&source_id))
        {
            self.geo_foundation_show_status(cx, "FOUNDATION_INPUT_HOLD", "The selected source ID is outside the currently selected Geo snapshot closure. Select a source included by the linked model/scenario.");
            return;
        }
        if !self.geo_foundation_ui.bedding_source_ids.contains(&source_id) {
            self.geo_foundation_ui.bedding_source_ids.push(source_id);
            self.geo_foundation_ui.bedding_source_ids.sort();
            self.geo_foundation_bedding_sources_changed(cx);
        }
    }

    fn geo_foundation_remove_bedding_source(&mut self, cx: &mut Cx) {
        let Some(source_id) = self.geo_foundation_ui.selected_bedding_source.clone() else {
            self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Choose a source ID before removing it from the bedding declaration.");
            return;
        };
        let before = self.geo_foundation_ui.bedding_source_ids.len();
        self.geo_foundation_ui.bedding_source_ids.retain(|id| id != &source_id);
        if self.geo_foundation_ui.bedding_source_ids.len() != before {
            self.geo_foundation_bedding_sources_changed(cx);
        }
    }

    fn geo_foundation_clear_bedding_declaration(&mut self, cx: &mut Cx) {
        let Some(id) = self.geo_foundation_ui.selected_placement.clone() else {
            self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Select a placement before clearing its bedding declaration.");
            return;
        };
        self.geo_foundation_ui.bedding_source_ids.clear();
        self.ui.text_input(cx, ids!(gf_manual_k_kn_m3)).set_text(cx, "");
        self.ui.text_input(cx, ids!(gf_foundation_bedding_note)).set_text(cx, "");
        self.geo_foundation_bedding_sources_changed(cx);
        self.geo_foundation_ui.bedding_clear_requested = Some(id);
    }

    fn geo_foundation_load_bedding_fields(&mut self, cx: &mut Cx, placement_id: Option<&FoundationId>) {
        self.geo_foundation_ui.bedding_source_ids.clear();
        self.geo_foundation_ui.selected_bedding_source = None;
        self.geo_foundation_ui.bedding_clear_requested = None;
        let assumption = self.geo_foundation_ui.projection.as_ref().and_then(|projection| {
            let id = placement_id?;
            projection.layout.as_ref()?.input.bedding_assumptions.get(id.as_str()).cloned()
        });
        match assumption {
            Some(foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform { k_kn_m3, assumption_note, source_ids }) => {
                self.ui.text_input(cx, ids!(gf_manual_k_kn_m3)).set_text(cx, &k_kn_m3.to_string());
                self.ui.text_input(cx, ids!(gf_foundation_bedding_note)).set_text(cx, &assumption_note);
                self.geo_foundation_ui.bedding_source_ids = source_ids;
                let text = self.geo_foundation_ui.bedding_source_ids.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
                self.ui.text_input(cx, ids!(gf_bedding_source_ids)).set_text(cx, &text);
                self.ui.text_input(cx, ids!(gf_layout_status)).set_text(cx, "Authored manual synthetic uniform bedding assumption loaded from FoundationLayout; source IDs must match linked Geo snapshot closure.");
            }
            Some(foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticPerIge { .. }) => {
                self.ui.text_input(cx, ids!(gf_manual_k_kn_m3)).set_text(cx, "");
                self.ui.text_input(cx, ids!(gf_foundation_bedding_note)).set_text(cx, "");
                self.ui.text_input(cx, ids!(gf_bedding_source_ids)).set_text(cx, "Per-IGE declaration retained; this editor does not rewrite it.");
                self.ui.text_input(cx, ids!(gf_layout_status)).set_text(cx, "An authored per-IGE bedding map is retained, but this conditional numerical adapter currently supports uniform bedding only.");
            }
            None => {
                self.ui.text_input(cx, ids!(gf_manual_k_kn_m3)).set_text(cx, "");
                self.ui.text_input(cx, ids!(gf_foundation_bedding_note)).set_text(cx, "");
                self.ui.text_input(cx, ids!(gf_bedding_source_ids)).set_text(cx, "No authored bedding sources selected");
            }
        }
        self.geo_foundation_ui.bedding_fields_dirty = false;
    }

    fn geo_foundation_guard_dirty_selection(&mut self, cx: &mut Cx, widget: &[LiveId], index: usize) -> bool {
        if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty {
            self.geo_foundation_restore_dropdown(cx, widget, index);
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload the current draft before changing this record selection; entered text is retained.");
            false
        } else {
            true
        }
    }

    fn geo_foundation_show_status(&mut self, cx: &mut Cx, code: &str, message: &str) {
        self.geo_foundation_ui.status = format!("{code} · {message}");
        self.geo_foundation_ui.set_attempt(Some(code), message);
        self.ui.label(cx, ids!(gf_status)).set_text(cx, &self.geo_foundation_ui.status);
        self.ui.label(cx, ids!(gf_attempt_status)).set_text(cx, message);
    }

    fn geo_foundation_handle_record_creation(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(gf_new_source)).clicked(actions) {
            let id = GeoFoundationUi::draft_id("source", self.geo_foundation_ui.source_ids.iter().map(|id| id.as_str()));
            let Ok(id) = SourceId::new(id) else { return; };
            self.geo_foundation_begin_record(cx, GeoFoundationTab::Sources, GeoFoundationDraft::Source(SourceRecord {
                id, title: String::new(), origin: geo_core::SourceOrigin::Synthetic, reference: String::new(),
                content_sha256: None, observed_at: None, revision: 0, source_page_or_record: None,
                source_date: None, source_revision: None,
            }));
            self.ui.drop_down(cx, ids!(gf_source_origin)).set_selected_item(cx, 0);
        } else if self.ui.button(cx, ids!(gf_new_soil)).clicked(actions) {
            let id = GeoFoundationUi::draft_id("soil", self.geo_foundation_ui.soil_ids.iter().map(|id| id.as_str()));
            if let Ok(id) = GeoSoilId::new(id) {
                self.geo_foundation_begin_record(cx, GeoFoundationTab::Soils, GeoFoundationDraft::Soil(GeoSoilRecord {
                    id, name: String::new(), parameters: SoilParameters::default(), revision: 0,
                }));
            }
        } else if self.ui.button(cx, ids!(gf_new_ige)).clicked(actions) {
            let id = GeoFoundationUi::draft_id("ige", self.geo_foundation_ui.ige_ids.iter().map(|id| id.as_str()));
            if let Ok(id) = IgeId::new(id) {
                let soil_id = self.geo_foundation_ui.selected_soil.clone().or_else(|| self.geo_foundation_ui.soil_ids.first().cloned());
                let Some(soil_id) = soil_id else { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Create/select a stored GeoSoilRecord before adding an IGE."); return; };
                self.geo_foundation_begin_record(cx, GeoFoundationTab::Iges, GeoFoundationDraft::Ige(IgeRecord { id, name: String::new(), soil_id, source_ids: Vec::new(), revision: 0 }));
            }
        } else if self.ui.button(cx, ids!(gf_new_borehole)).clicked(actions) {
            let id = GeoFoundationUi::draft_id("borehole", self.geo_foundation_ui.borehole_ids.iter().map(|id| id.as_str()));
            if let Ok(id) = BoreholeId::new(id) {
                self.geo_foundation_begin_record(cx, GeoFoundationTab::Boreholes, GeoFoundationDraft::Borehole(Borehole {
                    id, collar_m: [0.0; 3], source_ids: Vec::new(), intervals: Vec::new(), revision: 0,
                    total_depth_m: Parameter::Missing { reason: "NOT_PROVIDED".into() }, drilling_date: None,
                    crs: None, collar_vertical_datum: None, depth_datum: String::new(),
                    survey_source_ids: Vec::new(), evidence_source_ids: Vec::new(),
                }));
            }
        } else if self.ui.button(cx, ids!(gf_new_model)).clicked(actions) {
            let id = GeoFoundationUi::draft_id("model", self.geo_foundation_ui.model_ids.iter().map(|id| id.as_str()));
            if let Ok(id) = GroundModelId::new(id) {
                self.geo_foundation_begin_record(cx, GeoFoundationTab::Model, GeoFoundationDraft::Model(GroundModel {
                    id, name: String::new(), frame: GeoCoordinateFrame {
                        name: String::new(), crs: None, vertical_datum: None,
                        missing_borehole_crs: geo_core::MissingBoreholeCrsPolicy::Unspecified,
                    },
                    borehole_ids: Vec::new(), triangles: Vec::new(), contacts: Vec::new(), allow_pinch: false,
                    unknown_zones: Vec::new(), interpolation: InterpolationMethod::ExplicitTinPiecewisePlanar,
                    source_ids: Vec::new(), section_ids: Vec::new(), revision: 0,
                }));
            }
        } else if self.ui.button(cx, ids!(gf_new_scenario)).clicked(actions) {
            let id = GeoFoundationUi::draft_id("scenario", self.geo_foundation_ui.scenario_ids.iter().map(|id| id.as_str()));
            if let Ok(id) = GroundScenarioId::new(id) {
                let model_id = self.geo_foundation_ui.selected_model.clone().or_else(|| self.geo_foundation_ui.model_ids.first().cloned());
                let Some(model_id) = model_id else { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Create/select a stored ground model before adding a scenario."); return; };
                self.geo_foundation_ui.selected_scenario = Some(id.clone());
                self.geo_foundation_ui.selected_snapshot = None;
                self.geo_foundation_begin_record(cx, GeoFoundationTab::Water, GeoFoundationDraft::Scenario(GeoScenario {
                    id, name: String::new(), model_id, water: geo_core::WaterScenario::Unknown, source_ids: Vec::new(), revision: 0,
                }));
            }
        } else if self.ui.button(cx, ids!(gf_new_water_observation)).clicked(actions) {
            let id = GeoFoundationUi::draft_id("water", self.geo_foundation_ui.water_observation_ids.iter().map(|id| id.as_str()));
            if let Ok(id) = geo_core::WaterObservationId::new(id) {
                self.geo_foundation_ui.water_editing_observation = true;
                self.geo_foundation_ui.selected_water_observation = Some(id.clone());
                self.geo_foundation_begin_record(cx, GeoFoundationTab::Water, GeoFoundationDraft::WaterObservation(WaterObservation {
                    id, xy_m: [0.0; 2], elevation_abs_m: Parameter::Missing { reason: "NOT_PROVIDED".into() },
                    observed_at: None, revision: 0,
                }));
            }
        } else if self.ui.button(cx, ids!(gf_new_section)).clicked(actions) {
            let id = GeoFoundationUi::draft_id("section", self.geo_foundation_ui.section_ids.iter().map(|id| id.as_str()));
            if let Ok(id) = GeoSectionId::new(id) {
                let model_id = self.geo_foundation_ui.selected_model.clone().or_else(|| self.geo_foundation_ui.model_ids.first().cloned());
                let Some(model_id) = model_id else { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Create/select a ground model before adding an interpreted section."); return; };
                self.geo_foundation_begin_record(cx, GeoFoundationTab::Sections, GeoFoundationDraft::Section(InterpretedSection { id, model_id, polyline_xy_m: Vec::new(), source_ids: Vec::new(), revision: 0 }));
            }
        } else if self.ui.button(cx, ids!(gf_new_contact)).clicked(actions) {
            if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload the current draft before adding a contact horizon."); return; }
            let model = match self.geo_foundation_current_model() { Ok(model) => model, Err(error) => { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", &error); return; } };
            let horizon_id = GeoFoundationUi::draft_id("horizon", model.contacts.iter().map(|contact| contact.id.as_str()));
            self.geo_foundation_ui.active_tab = GeoFoundationTab::Contacts;
            self.geo_foundation_ui.selected_contact = geo_core::HorizonId::new(horizon_id.clone()).ok();
            self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Model(model));
            self.geo_foundation_ui.new_record = true;
            self.geo_foundation_ui.draft_dirty = true;
            self.geo_foundation_ui.binding_stale = false;
            self.geo_foundation_set_tab_host(cx);
            self.geo_foundation_set_input(cx, ids!(gf_contact_id), &horizon_id);
            self.geo_foundation_set_input(cx, ids!(gf_contact_vertical_datum), "");
            self.geo_foundation_set_input(cx, ids!(gf_contact_elevation), "");
            self.geo_foundation_set_input(cx, ids!(gf_contact_missing_reason), "");
            self.geo_foundation_set_input(cx, ids!(gf_contact_source_ids), "");
            self.geo_foundation_set_input(cx, ids!(gf_contact_candidates), "[]");
            self.ui.drop_down(cx, ids!(gf_contact_parameter_state)).set_selected_item(cx, 0);
            self.geo_foundation_ui.mark_draft_dirty();
            self.geo_foundation_clear_scene(cx);
            self.geo_foundation_show_status(cx, "INPUT_ONLY", "New ordered contact horizon draft · elevation values remain Missing until explicitly entered.");
        } else if self.ui.button(cx, ids!(gf_new_unknown)).clicked(actions) {
            if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload the current draft before adding an unknown zone."); return; }
            let model = match self.geo_foundation_current_model() { Ok(model) => model, Err(error) => { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", &error); return; } };
            let index = model.unknown_zones.len();
            self.geo_foundation_ui.active_tab = GeoFoundationTab::UnknownZones;
            self.geo_foundation_ui.unknown_zone_index = Some(index);
            self.geo_foundation_begin_record(cx, GeoFoundationTab::UnknownZones, GeoFoundationDraft::UnknownZone(UnknownZone { polygon_xy_m: Vec::new(), reason: String::new(), source_ids: Vec::new() }));
        } else if self.ui.button(cx, ids!(gf_model_borehole_add)).clicked(actions) {
            self.geo_foundation_add_model_borehole(cx);
        } else if self.ui.button(cx, ids!(gf_model_borehole_remove)).clicked(actions) {
            self.geo_foundation_remove_model_borehole(cx);
        } else if self.ui.button(cx, ids!(gf_new_triangle)).clicked(actions) {
            self.geo_foundation_add_triangle(cx);
        } else if self.ui.button(cx, ids!(gf_interval_add)).clicked(actions) {
            self.geo_foundation_add_interval(cx);
        } else if self.ui.button(cx, ids!(gf_interval_apply)).clicked(actions) {
            self.geo_foundation_apply_record(cx);
        }
    }

    fn geo_foundation_mark_model_draft(&mut self, cx: &mut Cx, model: GroundModel) {
        self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Model(model));
        self.geo_foundation_ui.mark_draft_dirty();
        self.geo_foundation_clear_scene(cx);
        self.geo_foundation_sync_selectors(cx);
        self.ui.button(cx, ids!(gf_apply)).set_enabled(cx, true);
        self.geo_foundation_show_status(cx, "INPUT_ONLY", "Unapplied GroundModel topology draft · explicit TIN connectivity remains source-bound.");
    }

    fn geo_foundation_add_model_borehole(&mut self, cx: &mut Cx) {
        let mut model = match self.geo_foundation_ui.draft.as_ref() {
            Some(GeoFoundationDraft::Model(model)) => model.clone(),
            _ => match self.geo_foundation_current_model() { Ok(model) => model, Err(error) => { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", &error); return; } },
        };
        let Some(id) = self.geo_foundation_ui.borehole_ids.get(self.ui.drop_down(cx, ids!(gf_model_borehole_select)).selected_item()).cloned() else {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Select an existing Borehole record before adding it to the model."); return;
        };
        if model.borehole_ids.contains(&id) { self.geo_foundation_show_status(cx, "INPUT_ONLY", "This borehole is already part of the model."); return; }
        model.borehole_ids.push(id);
        self.geo_foundation_mark_model_draft(cx, model);
    }

    fn geo_foundation_remove_model_borehole(&mut self, cx: &mut Cx) {
        let mut model = match self.geo_foundation_ui.draft.as_ref() {
            Some(GeoFoundationDraft::Model(model)) => model.clone(),
            _ => match self.geo_foundation_current_model() { Ok(model) => model, Err(error) => { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", &error); return; } },
        };
        let Some(id) = self.geo_foundation_ui.borehole_ids.get(self.ui.drop_down(cx, ids!(gf_model_borehole_select)).selected_item()).cloned() else {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Select a model Borehole to remove."); return;
        };
        let old_len = model.borehole_ids.len();
        model.borehole_ids.retain(|value| value != &id);
        if model.borehole_ids.len() == old_len { self.geo_foundation_show_status(cx, "INPUT_ONLY", "The selected borehole was not in this model."); return; }
        self.geo_foundation_mark_model_draft(cx, model);
    }

    fn geo_foundation_add_triangle(&mut self, cx: &mut Cx) {
        let mut model = match self.geo_foundation_ui.draft.as_ref() {
            Some(GeoFoundationDraft::Model(model)) => model.clone(),
            _ => match self.geo_foundation_current_model() { Ok(model) => model, Err(error) => { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", &error); return; } },
        };
        let id_text = self.ui.text_input(cx, ids!(gf_triangle_id)).text();
        let id = match geo_core::TriangleId::new(id_text) { Ok(id) => id, Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &format!("TriangleId: {error}")); return; } };
        if model.triangles.iter().any(|triangle| triangle.id == id) { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Triangle ID is already used in this model."); return; }
        let vertex = |widget| self.geo_foundation_ui.model_borehole_ids.get(self.ui.drop_down(cx, widget).selected_item()).cloned();
        let vertices = [vertex(ids!(gf_triangle_vertex_a)), vertex(ids!(gf_triangle_vertex_b)), vertex(ids!(gf_triangle_vertex_c))];
        let (Some(a), Some(b), Some(c)) = (vertices[0].clone(), vertices[1].clone(), vertices[2].clone()) else { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Choose three model boreholes for the explicit TIN triangle."); return; };
        if a == b || b == c || a == c { self.geo_foundation_show_status(cx, "INVALID_INPUT", "A TIN triangle needs three distinct model boreholes."); return; }
        let source_ids = match parse_source_ids(&self.ui.text_input(cx, ids!(gf_triangle_source_ids)).text()) { Ok(ids) if !ids.is_empty() => ids, Ok(_) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Explicit triangle connectivity requires source IDs."); return; }, Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &error); return; } };
        model.triangles.push(GeoTriangle { id: id.clone(), vertices: [a,b,c], source_ids });
        self.geo_foundation_ui.selected_triangle = Some(id.clone());
        self.geo_foundation_mark_model_draft(cx, model);
        self.geo_foundation_set_input(cx, ids!(gf_triangle_id), id.as_str());
    }

    fn geo_foundation_add_interval(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.binding_stale { self.geo_foundation_show_status(cx, "STALE_BINDING", "Reload before editing this Borehole interval."); return; }
        let Some(GeoFoundationDraft::Borehole(mut borehole)) = self.geo_foundation_ui.draft.clone() else { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Select or create a Borehole before adding an interval."); return; };
        let id_text = GeoFoundationUi::draft_id("interval", borehole.intervals.iter().map(|interval| interval.id.as_str()));
        let id = match geo_core::BoreholeIntervalId::new(id_text) { Ok(id) => id, Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &error.to_string()); return; } };
        let Some(ige_id) = self.geo_foundation_ui.ige_ids.get(self.ui.drop_down(cx, ids!(gf_interval_ige)).selected_item()).cloned() else { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Create/select an IGE before adding a borehole interval."); return; };
        borehole.intervals.push(geo_core::BoreholeInterval { id: id.clone(), top_depth_m: 0.0, bottom_depth_m: 0.0, ige_id, source_ids: Vec::new() });
        self.geo_foundation_ui.selected_interval = Some(id.clone());
        self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Borehole(borehole));
        self.geo_foundation_ui.mark_draft_dirty();
        self.geo_foundation_sync_selectors(cx);
        self.geo_foundation_set_input(cx, ids!(gf_interval_id), id.as_str());
        self.geo_foundation_set_input(cx, ids!(gf_interval_top), "");
        self.geo_foundation_set_input(cx, ids!(gf_interval_bottom), "");
        self.geo_foundation_set_input(cx, ids!(gf_interval_source_ids), "");
        self.geo_foundation_clear_scene(cx);
        self.geo_foundation_show_status(cx, "INPUT_ONLY", "Unapplied borehole interval draft; exact top/bottom depths and source IDs are required.");
    }

    fn geo_foundation_begin_record(&mut self, cx: &mut Cx, tab: GeoFoundationTab, draft: GeoFoundationDraft) {
        if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload the existing draft before creating another record.");
            return;
        }
        self.geo_foundation_ui.active_tab = tab;
        self.geo_foundation_ui.draft = None;
        self.geo_foundation_ui.new_record = true;
        self.geo_foundation_ui.draft_dirty = true;
        self.geo_foundation_ui.binding_stale = false;
        self.geo_foundation_set_tab_host(cx);
        self.geo_foundation_clear_editor(cx);
        self.geo_foundation_ui.draft = Some(draft);
        self.geo_foundation_fill_new_record(cx);
        self.geo_foundation_show_status(cx, "INPUT_ONLY", "New source-bound record draft · values are not stored until Apply.");
        self.geo_foundation_apply_visibility(cx);
    }

    fn geo_foundation_fill_new_record(&mut self, cx: &mut Cx) {
        let Some(draft) = self.geo_foundation_ui.draft.clone() else { return; };
        match draft {
            GeoFoundationDraft::Source(record) => {
                self.geo_foundation_set_input(cx, ids!(gf_source_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_source_title), "");
                self.geo_foundation_set_input(cx, ids!(gf_source_reference), "");
            }
            GeoFoundationDraft::Soil(record) => {
                self.geo_foundation_set_input(cx, ids!(gf_soil_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_soil_name), "");
                let index = self.ui.drop_down(cx, ids!(gf_soil_parameter)).selected_item().min(4);
                let (status, value, reason, sources, candidates) = parameter_editor_fields(soil_parameter_at(&record.parameters, index));
                self.ui.drop_down(cx, ids!(gf_parameter_state)).set_selected_item(cx, status);
                for (widget, text) in [(ids!(gf_parameter_value), value), (ids!(gf_parameter_missing_reason), reason), (ids!(gf_parameter_source_ids), sources), (ids!(gf_parameter_candidates), candidates)] {
                    self.geo_foundation_set_input(cx, widget, &text);
                }
            }
            GeoFoundationDraft::Ige(record) => {
                self.geo_foundation_set_input(cx, ids!(gf_ige_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_ige_name), "");
                self.geo_foundation_set_input(cx, ids!(gf_ige_source_ids), "");
                if let Some(index) = self.geo_foundation_ui.soil_ids.iter().position(|id| id == &record.soil_id) { self.ui.drop_down(cx, ids!(gf_ige_soil)).set_selected_item(cx, index); }
            }
            GeoFoundationDraft::Borehole(record) => {
                self.geo_foundation_set_input(cx, ids!(gf_borehole_id), record.id.as_str());
                for widget in [ids!(gf_collar_x),ids!(gf_collar_y),ids!(gf_collar_z),ids!(gf_borehole_collar_vertical_datum),ids!(gf_borehole_depth_datum)] { self.geo_foundation_set_input(cx, widget, ""); }
                self.geo_foundation_set_input(cx, ids!(gf_borehole_source_ids), "");
            }
            GeoFoundationDraft::Model(record) => {
                self.geo_foundation_set_input(cx, ids!(gf_ground_model_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_ground_model_name), "");
                self.geo_foundation_set_input(cx, ids!(gf_frame_name), "");
                self.geo_foundation_set_input(cx, ids!(gf_model_source_ids), "");
            }
            GeoFoundationDraft::Scenario(record) => {
                self.geo_foundation_set_input(cx, ids!(gf_scenario_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_scenario_name), "");
                self.geo_foundation_set_input(cx, ids!(gf_scenario_source_ids), "");
                self.ui.drop_down(cx, ids!(gf_water_kind)).set_selected_item(cx, 0);
            }
            GeoFoundationDraft::WaterObservation(record) => {
                self.geo_foundation_set_input(cx, ids!(gf_water_observation_id), record.id.as_str());
                for widget in [ids!(gf_water_x),ids!(gf_water_y)] { self.geo_foundation_set_input(cx, widget, ""); }
            }
            GeoFoundationDraft::Section(record) => {
                self.geo_foundation_set_input(cx, ids!(gf_section_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_section_alignment), "");
                self.geo_foundation_set_input(cx, ids!(gf_section_source_ids), "");
            }
            GeoFoundationDraft::UnknownZone(_) => {
                self.geo_foundation_set_input(cx, ids!(gf_unknown_polygon), "");
                self.geo_foundation_set_input(cx, ids!(gf_unknown_reason), "");
                self.geo_foundation_set_input(cx, ids!(gf_unknown_source_ids), "");
            }
        }
    }

    fn geo_foundation_handle_view_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for (widget, tab) in [
            (ids!(gf_tab_sources), GeoFoundationTab::Sources),
            (ids!(gf_tab_soils), GeoFoundationTab::Soils),
            (ids!(gf_tab_iges), GeoFoundationTab::Iges),
            (ids!(gf_tab_boreholes), GeoFoundationTab::Boreholes),
            (ids!(gf_tab_model), GeoFoundationTab::Model),
            (ids!(gf_tab_contacts), GeoFoundationTab::Contacts),
            (ids!(gf_tab_water), GeoFoundationTab::Water),
            (ids!(gf_tab_sections), GeoFoundationTab::Sections),
            (ids!(gf_tab_unknown), GeoFoundationTab::UnknownZones),
        ] {
            if self.ui.button(cx, widget).clicked(actions) {
                if self.geo_foundation_ui.draft_dirty {
                    self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload the current record before switching editor tabs; text is retained.");
                } else {
                    self.geo_foundation_ui.active_tab = tab;
                    self.geo_foundation_ui.new_record = false;
                    self.geo_foundation_set_tab_host(cx);
                    self.geo_foundation_load_selected_editor(cx);
                    self.geo_foundation_update_record_inspector(cx);
                }
                return;
            }
        }
        for (widget, view) in [
            (ids!(gf_view_plan), GeoFoundationView::GroundPlan),
            (ids!(gf_view_section), GeoFoundationView::Section),
            (ids!(gf_view_3d), GeoFoundationView::Ground3D),
            (ids!(gf_view_foundation_plan), GeoFoundationView::FoundationPlan),
            (ids!(gf_view_foundation_3d), GeoFoundationView::Foundation3D),
            (ids!(gf_view_pressure), GeoFoundationView::Pressure2D),
            (ids!(gf_view_settlement), GeoFoundationView::Settlement2D),
        ] {
            if self.ui.button(cx, widget).clicked(actions) {
                self.geo_foundation_ui.view = view;
                self.geo_foundation_ui.pan_anchor = None;
                self.geo_foundation_ui.fit_pending = true;
                self.geo_foundation_render_changed(cx, false);
                self.geo_foundation_apply_visibility(cx);
                return;
            }
        }
        if self.ui.button(cx, ids!(gf_fit)).clicked(actions) {
            self.geo_foundation_ui.fit_pending = true;
            self.geo_foundation_render_changed(cx, true);
            return;
        }
        if let Some(value) = self.ui.check_box(cx, ids!(gf_show_unknown)).changed(actions) { self.geo_foundation_ui.show_unknown_zones = value; self.geo_foundation_render_changed(cx, false); }
        if let Some(value) = self.ui.check_box(cx, ids!(gf_show_terrain)).changed(actions) { self.geo_foundation_ui.show_terrain = value; self.geo_foundation_render_changed(cx, false); }
        if let Some(value) = self.ui.check_box(cx, ids!(gf_show_contacts)).changed(actions) { self.geo_foundation_ui.show_contacts = value; self.geo_foundation_render_changed(cx, false); }
        if let Some(value) = self.ui.check_box(cx, ids!(gf_show_boreholes)).changed(actions) { self.geo_foundation_ui.show_boreholes = value; self.geo_foundation_render_changed(cx, false); }
        if let Some(value) = self.ui.check_box(cx, ids!(gf_show_water)).changed(actions) { self.geo_foundation_ui.show_water = value; self.geo_foundation_render_changed(cx, false); }
        if let Some(value) = self.ui.check_box(cx, ids!(gf_show_foundations)).changed(actions) { self.geo_foundation_ui.show_foundations = value; self.geo_foundation_render_changed(cx, false); }
        if let Some(text) = self.ui.text_input(cx, ids!(gf_section_spacing)).changed(actions) {
            match parse_finite_field(&text, "Section sample spacing") {
                Ok(value) if value > 0.0 && value <= 100_000.0 => {
                    self.geo_foundation_ui.section_max_spacing_m = value;
                    self.geo_foundation_ui.scene_key = None;
                    self.geo_foundation_ui.screen_key = None;
                    self.geo_foundation_render_changed(cx, false);
                }
                Ok(_) => self.geo_foundation_show_status(cx, "INVALID_INPUT", "Section sample spacing must be greater than 0 and at most 100000 m."),
                Err(error) => self.geo_foundation_show_status(cx, "INVALID_INPUT", &error),
            }
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_visible_ige)).selected(actions) {
            self.geo_foundation_ui.visible_ige_filter = if index == 0 { None } else { self.geo_foundation_ui.ige_ids.get(index - 1).cloned() };
            self.geo_foundation_ui.scene_key = None;
            self.geo_foundation_ui.screen_key = None;
            self.geo_foundation_render_changed(cx, false);
        }
    }

    fn geo_foundation_handle_input_changes(&mut self, cx: &mut Cx, actions: &Actions) {
        let text_inputs = [
            id!(gf_source_id), id!(gf_source_title), id!(gf_source_reference), id!(gf_source_sha256), id!(gf_source_observed_at),
            id!(gf_soil_id), id!(gf_soil_name), id!(gf_parameter_value), id!(gf_parameter_missing_reason), id!(gf_parameter_source_ids), id!(gf_parameter_candidates),
            id!(gf_ige_id), id!(gf_ige_name), id!(gf_ige_source_ids), id!(gf_borehole_id), id!(gf_collar_x), id!(gf_collar_y), id!(gf_collar_z), id!(gf_borehole_collar_vertical_datum),
            id!(gf_borehole_source_ids), id!(gf_borehole_total_depth), id!(gf_borehole_total_depth_reason), id!(gf_borehole_total_depth_sources),
            id!(gf_borehole_date), id!(gf_borehole_crs), id!(gf_borehole_depth_datum), id!(gf_borehole_survey_sources), id!(gf_borehole_evidence_sources),
            id!(gf_interval_id), id!(gf_interval_top), id!(gf_interval_bottom), id!(gf_interval_source_ids),
            id!(gf_ground_model_id), id!(gf_ground_model_name), id!(gf_frame_name), id!(gf_frame_crs), id!(gf_frame_datum), id!(gf_model_source_ids),
            id!(gf_triangle_id), id!(gf_triangle_source_ids), id!(gf_contact_id), id!(gf_contact_vertical_datum), id!(gf_contact_elevation), id!(gf_contact_missing_reason), id!(gf_contact_source_ids), id!(gf_contact_candidates),
            id!(gf_unknown_polygon), id!(gf_unknown_reason), id!(gf_unknown_source_ids), id!(gf_scenario_id), id!(gf_scenario_name), id!(gf_scenario_source_ids),
            id!(gf_water_source_ids), id!(gf_water_observation_ids_text), id!(gf_water_observation_id), id!(gf_water_x), id!(gf_water_y), id!(gf_water_observed_at),
            id!(gf_water_elevation), id!(gf_water_elevation_source_ids), id!(gf_section_id), id!(gf_section_alignment), id!(gf_section_source_ids),
        ];
        let mut changed = text_inputs.into_iter().any(|id| self.ui.text_input(cx, &[id]).changed(actions).is_some());
        for (widget, is_edit) in [
            (ids!(gf_source_origin), self.geo_foundation_ui.active_tab == GeoFoundationTab::Sources),
            (ids!(gf_soil_parameter), self.geo_foundation_ui.active_tab == GeoFoundationTab::Soils),
            (ids!(gf_parameter_state), self.geo_foundation_ui.active_tab == GeoFoundationTab::Soils),
            (ids!(gf_ige_soil), self.geo_foundation_ui.active_tab == GeoFoundationTab::Iges),
            (ids!(gf_borehole_total_depth_state), self.geo_foundation_ui.active_tab == GeoFoundationTab::Boreholes),
            (ids!(gf_interval_ige), self.geo_foundation_ui.active_tab == GeoFoundationTab::Boreholes),
            (ids!(gf_allow_pinch), self.geo_foundation_ui.active_tab == GeoFoundationTab::Model),
            (ids!(gf_triangle_vertex_a), self.geo_foundation_ui.active_tab == GeoFoundationTab::Model),
            (ids!(gf_triangle_vertex_b), self.geo_foundation_ui.active_tab == GeoFoundationTab::Model),
            (ids!(gf_triangle_vertex_c), self.geo_foundation_ui.active_tab == GeoFoundationTab::Model),
            (ids!(gf_contact_ige), self.geo_foundation_ui.active_tab == GeoFoundationTab::Contacts),
            (ids!(gf_contact_borehole), self.geo_foundation_ui.active_tab == GeoFoundationTab::Contacts),
            (ids!(gf_contact_parameter_state), self.geo_foundation_ui.active_tab == GeoFoundationTab::Contacts),
            (ids!(gf_scenario_model), self.geo_foundation_ui.active_tab == GeoFoundationTab::Water),
            (ids!(gf_water_kind), self.geo_foundation_ui.active_tab == GeoFoundationTab::Water),
            (ids!(gf_water_elevation_state), self.geo_foundation_ui.active_tab == GeoFoundationTab::Water),
            (ids!(gf_section_model), self.geo_foundation_ui.active_tab == GeoFoundationTab::Sections),
        ] {
            if is_edit && self.ui.drop_down(cx, widget).changed(actions).is_some() { changed = true; }
        }
        if self.geo_foundation_ui.active_tab == GeoFoundationTab::Soils {
            if let Some(index) = self.ui.drop_down(cx, ids!(gf_soil_parameter)).selected(actions) {
                if self.geo_foundation_ui.draft_dirty {
                    self.ui.drop_down(cx, ids!(gf_soil_parameter)).set_selected_item(cx, self.geo_foundation_ui.selected_parameter_index);
                    self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply the current soil parameter edit before switching parameter rows; text is retained.");
                } else {
                    self.geo_foundation_ui.selected_parameter_index = index.min(4);
                    self.geo_foundation_load_selected_editor(cx);
                }
                return;
            }
        }
        if changed {
            if self.geo_foundation_ui.binding.is_none() || self.document.is_none() {
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Open a Document and reload before editing. The visible text is not bound to a source record.");
            } else {
                self.geo_foundation_ui.mark_draft_dirty();
                self.geo_foundation_ui.attempt_status = Some("INPUT_ONLY".into());
                self.geo_foundation_ui.attempt_message = "Unapplied typed editor draft · previous derived layers are hidden.".into();
                self.geo_foundation_clear_scene(cx);
                self.ui.label(cx, ids!(gf_attempt_status)).set_text(cx, &self.geo_foundation_ui.attempt_message);
                self.ui.button(cx, ids!(gf_apply)).set_enabled(cx, true);
                self.ui.button(cx, ids!(gf_save_snapshot)).set_enabled(cx, false);
                self.ui.button(cx, ids!(gf_foundation_analyze)).set_enabled(cx, false);
            }
        }
    }

    fn geo_foundation_handle_layout_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_placement_select)).selected(actions) {
            if self.geo_foundation_ui.layout_fields_dirty || self.geo_foundation_ui.analysis_controls_dirty || self.geo_foundation_ui.analysis_controls_stale {
                let current = selector_index(&self.geo_foundation_ui.placement_ids, self.geo_foundation_ui.selected_placement.as_ref());
                self.geo_foundation_restore_dropdown(cx, ids!(gf_placement_select), current);
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Resolve or reload the layout/numerical-control draft before changing placement; text is retained.");
            } else {
                self.geo_foundation_clear_analysis_controls(cx);
                self.geo_foundation_ui.ground_map = None;
                self.geo_foundation_ui.ground_map_key = None;
                self.geo_foundation_ui.ground_map_status = "Placement changed · request a new source-bound midpoint map explicitly.".into();
                self.geo_foundation_ui.selected_placement = self.geo_foundation_ui.placement_ids.get(index).cloned();
                self.geo_foundation_ui.selected_foundation = self.geo_foundation_ui.selected_placement.as_ref().map(ToString::to_string);
                self.geo_foundation_sync_foundation_selectors(cx);
                self.geo_foundation_load_layout_form(cx);
            }
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_tier_select)).selected(actions) {
            if self.geo_foundation_ui.layout_fields_dirty {
                self.geo_foundation_restore_dropdown(cx, ids!(gf_tier_select), selector_index(&self.geo_foundation_ui.tier_ids, self.geo_foundation_ui.selected_tier_id.as_ref()));
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Save or reload the layout draft before changing tier; text is retained.");
            } else {
                self.geo_foundation_ui.selected_tier_id = self.geo_foundation_ui.tier_ids.get(index).cloned();
                self.geo_foundation_load_layout_form(cx);
            }
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_ground_link_model)).selected(actions) {
            if self.geo_foundation_ui.analysis_controls_dirty || self.geo_foundation_ui.analysis_controls_stale {
                self.geo_foundation_restore_dropdown(cx, ids!(gf_ground_link_model), self.geo_foundation_ui.layout_link_model.as_ref()
                    .map_or(0, |id| selector_index(&self.geo_foundation_ui.ground_link_model_ids, Some(id)) + 1));
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Analyze or reload the current numerical-control draft before changing the foundation GEO link.");
                return;
            }
            self.geo_foundation_clear_analysis_controls(cx);
            self.geo_foundation_ui.layout_link_model = index.checked_sub(1).and_then(|index| self.geo_foundation_ui.ground_link_model_ids.get(index).cloned());
            self.geo_foundation_ui.layout_link_scenario = None;
            self.geo_foundation_sync_foundation_selectors(cx);
            self.geo_foundation_mark_layout_dirty(cx);
            return;
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gf_ground_link_scenario)).selected(actions) {
            if self.geo_foundation_ui.analysis_controls_dirty || self.geo_foundation_ui.analysis_controls_stale {
                self.geo_foundation_restore_dropdown(cx, ids!(gf_ground_link_scenario), self.geo_foundation_ui.layout_link_scenario.as_ref()
                    .map_or(0, |id| selector_index(&self.geo_foundation_ui.ground_link_scenario_ids, Some(id)) + 1));
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Analyze or reload the current numerical-control draft before changing the foundation GEO link.");
                return;
            }
            self.geo_foundation_clear_analysis_controls(cx);
            self.geo_foundation_ui.layout_link_scenario = index.checked_sub(1).and_then(|index| self.geo_foundation_ui.ground_link_scenario_ids.get(index).cloned());
            self.geo_foundation_mark_layout_dirty(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_placement_add)).clicked(actions) {
            self.geo_foundation_add_placement(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_placement_delete)).clicked(actions) {
            self.geo_foundation_delete_placement(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_tier_add)).clicked(actions) {
            self.geo_foundation_add_tier(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_tier_delete)).clicked(actions) {
            self.geo_foundation_delete_tier(cx);
            return;
        }
        if self.ui.button(cx, ids!(gf_ground_link_apply)).clicked(actions) {
            self.geo_foundation_apply_ground_link(cx);
            return;
        }
        let mut changed = false;
        for id in [
            id!(gf_layout_id), id!(gf_placement_id), id!(gf_placement_x), id!(gf_placement_y), id!(gf_placement_z),
            id!(gf_placement_length_x), id!(gf_placement_width_y), id!(gf_placement_thickness), id!(gf_placement_strip_length),
            id!(gf_placement_strip_width), id!(gf_placement_rotation), id!(gf_tier_id), id!(gf_tier_x), id!(gf_tier_y), id!(gf_tier_z),
            id!(gf_tier_length_x), id!(gf_tier_width_y), id!(gf_tier_thickness),
        ] {
            changed |= self.ui.text_input(cx, &[id]).changed(actions).is_some();
        }
        changed |= self.ui.drop_down(cx, ids!(gf_placement_kind)).changed(actions).is_some();
        if changed { self.geo_foundation_mark_layout_fields_dirty(cx); }
    }

    fn geo_foundation_mark_layout_dirty(&mut self, cx: &mut Cx) {
        self.geo_foundation_ui.layout_dirty = true;
        self.geo_foundation_ui.ground_map = None;
        self.geo_foundation_ui.ground_map_key = None;
        self.geo_foundation_ui.ground_map_status = "FoundationLayout draft changed · previous midpoint map cleared.".into();
        self.geo_foundation_ui.scene_key = None;
        self.geo_foundation_ui.screen_key = None;
        self.geo_foundation_ui.selected_foundation_snapshot = None;
        self.geo_foundation_clear_scene(cx);
        self.ui.button(cx, ids!(gf_layout_save)).set_enabled(cx, true);
        self.ui.button(cx, ids!(gf_foundation_analyze)).set_enabled(cx, false);
        self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
        self.geo_foundation_show_status(cx, "INPUT_ONLY", "Unapplied FoundationLayout draft · linked result layers are hidden until saved.");
    }

    fn geo_foundation_mark_layout_fields_dirty(&mut self, cx: &mut Cx) {
        self.geo_foundation_ui.layout_fields_dirty = true;
        self.geo_foundation_mark_layout_dirty(cx);
    }

    fn geo_foundation_capture_layout_fields(&mut self, cx: &mut Cx) -> bool {
        if !self.geo_foundation_ui.layout_fields_dirty { return true; }
        match self.geo_foundation_read_layout_form(cx) {
            Ok(input) => {
                let previous_placement = self.geo_foundation_ui.selected_placement.clone();
                let next_placement = FoundationId::new(self.ui.text_input(cx, ids!(gf_placement_id)).text()).ok();
                let previous_tier = self.geo_foundation_ui.selected_tier_id.clone();
                let next_tier = self.ui.text_input(cx, ids!(gf_tier_id)).text().trim().to_owned();
                self.geo_foundation_ui.layout_draft = Some(input.layout);
                if let (Some(previous), Some(next)) = (previous_placement, next_placement) {
                    let was_new = self.geo_foundation_ui.new_placement_ids.remove(&previous);
                    if was_new { self.geo_foundation_ui.new_placement_ids.insert(next.clone()); }
                    self.geo_foundation_ui.selected_placement = Some(next.clone());
                    self.geo_foundation_ui.selected_foundation = Some(next.to_string());
                }
                if let Some(previous) = previous_tier {
                    let was_new = self.geo_foundation_ui.new_tier_ids.remove(&previous);
                    if was_new && !next_tier.is_empty() { self.geo_foundation_ui.new_tier_ids.insert(next_tier.clone()); }
                    self.geo_foundation_ui.selected_tier_id = (!next_tier.is_empty()).then_some(next_tier);
                }
                self.geo_foundation_ui.layout_fields_dirty = false;
                true
            }
            Err(error) => {
                self.geo_foundation_show_status(cx, "INVALID_INPUT", &format!("Resolve the current layout fields before changing selection: {error}"));
                false
            }
        }
    }

    fn geo_foundation_apply_record(&mut self, cx: &mut Cx) {
        let Some(binding) = self.geo_foundation_ui.binding.clone() else {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Reload a current Document projection before applying a record.");
            return;
        };
        if self.geo_foundation_ui.binding_stale {
            self.geo_foundation_show_status(cx, "STALE_BINDING", "This draft is bound to an older Document state. Reload before applying; the text is retained.");
            return;
        }
        let draft = match self.geo_foundation_ui.draft.clone() {
            Some(draft) => draft,
            None if matches!(self.geo_foundation_ui.active_tab, GeoFoundationTab::Contacts | GeoFoundationTab::UnknownZones) => {
                match self.geo_foundation_current_model() {
                    Ok(model) => GeoFoundationDraft::Model(model),
                    Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &error); return; }
                }
            }
            None => {
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Select or create a typed Geo record before Apply.");
                return;
            }
        };
        let result = self.geo_foundation_build_command(cx, draft);
        let command = match result {
            Ok(command) => command,
            Err(error) => {
                self.geo_foundation_ui.draft_dirty = true;
                self.geo_foundation_clear_scene(cx);
                self.geo_foundation_show_status(cx, "INVALID_INPUT", &error);
                return;
            }
        };
        let section_edit = match &command {
            GeoCommand::UpsertSection { record } => Some(record.clone()),
            _ => None,
        };
        let section_commands = if let Some(section) = section_edit {
            let Some(projection) = self.geo_foundation_ui.projection.as_ref() else {
                self.geo_foundation_show_status(
                    cx,
                    "MODEL_INCOMPLETE",
                    "HOLD · section ownership update requires the current GEO projection; reload before applying.",
                );
                return;
            };
            match gf_api::section_edit_commands(&projection.state, section) {
                Ok(commands) => Some(commands),
                Err(error) => {
                    self.geo_foundation_show_status(cx, "INVALID_INPUT", &error);
                    return;
                }
            }
        } else {
            None
        };
        match &command {
            GeoCommand::UpsertSource { record } => self.geo_foundation_ui.selected_source = Some(record.id.clone()),
            GeoCommand::UpsertSoil { record } => self.geo_foundation_ui.selected_soil = Some(record.id.clone()),
            GeoCommand::UpsertIge { record } => self.geo_foundation_ui.selected_ige = Some(record.id.clone()),
            GeoCommand::UpsertBorehole { record } => self.geo_foundation_ui.selected_borehole = Some(record.id.clone()),
            GeoCommand::UpsertModel { record } => self.geo_foundation_ui.selected_model = Some(record.id.clone()),
            GeoCommand::UpsertScenario { record } => self.geo_foundation_ui.selected_scenario = Some(record.id.clone()),
            GeoCommand::UpsertWaterObservation { record } => self.geo_foundation_ui.selected_water_observation = Some(record.id.clone()),
            GeoCommand::UpsertSection { record } => self.geo_foundation_ui.selected_section = Some(record.id.clone()),
            GeoCommand::ImportRecords { .. } | GeoCommand::SaveSnapshot { .. } => {}
        }
        let commands = section_commands.unwrap_or_else(|| vec![command]);
        self.geo_foundation_ui.selected_snapshot = None;
        self.geo_foundation_ui.attempt_status = None;
        self.geo_foundation_ui.attempt_message.clear();
        self.geo_foundation_ui.draft_dirty = false;
        self.geo_foundation_ui.new_record = false;
        self.geo_foundation_clear_scene(cx);
        match self.geo_foundation_execute(cx, commands, binding) {
            Ok(()) => {
                self.geo_foundation_ui.draft = None;
                self.geo_foundation_refresh_projection(cx);
                self.geo_foundation_render_changed(cx, false);
            }
            Err(error) => {
                self.geo_foundation_ui.draft_dirty = true;
                let code = if error.starts_with("STALE_DRAFT") { "STALE_BINDING" } else { "INVALID_INPUT" };
                self.geo_foundation_show_status(cx, code, &error);
            }
        }
        self.geo_foundation_apply_visibility(cx);
    }

    fn geo_foundation_build_command(&self, cx: &Cx, draft: GeoFoundationDraft) -> Result<GeoCommand, String> {
        let new_record = self.geo_foundation_ui.new_record;
        match (self.geo_foundation_ui.active_tab, draft) {
            (GeoFoundationTab::Sources, GeoFoundationDraft::Source(mut record)) => {
                let id = SourceId::new(self.ui.text_input(cx, ids!(gf_source_id)).text()).map_err(|error| format!("Source ID: {error}"))?;
                ensure_record_id(&record.id, &id, new_record, "Source")?;
                let origin_index = self.ui.drop_down(cx, ids!(gf_source_origin)).selected_item();
                record.origin = match origin_index {
                    0 => return Err("Select an explicit source origin before applying.".into()),
                    1 => geo_core::SourceOrigin::Synthetic,
                    2 => geo_core::SourceOrigin::Measured,
                    3 => geo_core::SourceOrigin::Interpreted,
                    4 => geo_core::SourceOrigin::ImportedLegacy,
                    _ => return Err("Unknown source origin selection.".into()),
                };
                record.id = id;
                record.title = self.ui.text_input(cx, ids!(gf_source_title)).text().trim().to_owned();
                record.reference = self.ui.text_input(cx, ids!(gf_source_reference)).text().trim().to_owned();
                record.content_sha256 = optional_field(self.ui.text_input(cx, ids!(gf_source_sha256)).text());
                record.observed_at = optional_field(self.ui.text_input(cx, ids!(gf_source_observed_at)).text());
                Ok(GeoCommand::UpsertSource { record })
            }
            (GeoFoundationTab::Soils, GeoFoundationDraft::Soil(mut record)) => {
                let id = GeoSoilId::new(self.ui.text_input(cx, ids!(gf_soil_id)).text()).map_err(|error| format!("GeoSoilId: {error}"))?;
                ensure_record_id(&record.id, &id, new_record, "GeoSoilRecord")?;
                record.id = id;
                record.name = self.ui.text_input(cx, ids!(gf_soil_name)).text().trim().to_owned();
                let parameter_index = self.geo_foundation_ui.selected_parameter_index.min(4);
                let previous = soil_parameter_at(&record.parameters, parameter_index);
                let unit = match parameter_index {
                    0 => ParameterUnit::KnPerM3,
                    1 | 3 => ParameterUnit::Kpa,
                    2 => ParameterUnit::Degree,
                    _ => ParameterUnit::Dimensionless,
                };
                let parameter = parameter_from_fields(
                    self.ui.drop_down(cx, ids!(gf_parameter_state)).selected_item().min(2),
                    &self.ui.text_input(cx, ids!(gf_parameter_value)).text(),
                    &self.ui.text_input(cx, ids!(gf_parameter_missing_reason)).text(),
                    &self.ui.text_input(cx, ids!(gf_parameter_source_ids)).text(),
                    &self.ui.text_input(cx, ids!(gf_parameter_candidates)).text(),
                    unit, Some(previous),
                )?;
                replace_soil_parameter(&mut record.parameters, parameter_index, parameter);
                Ok(GeoCommand::UpsertSoil { record })
            }
            (GeoFoundationTab::Iges, GeoFoundationDraft::Ige(mut record)) => {
                let id = IgeId::new(self.ui.text_input(cx, ids!(gf_ige_id)).text()).map_err(|error| format!("IgeId: {error}"))?;
                ensure_record_id(&record.id, &id, new_record, "IGE")?;
                record.id = id;
                record.name = self.ui.text_input(cx, ids!(gf_ige_name)).text().trim().to_owned();
                let soil_index = self.ui.drop_down(cx, ids!(gf_ige_soil)).selected_item();
                record.soil_id = self.geo_foundation_ui.soil_ids.get(soil_index).cloned().ok_or_else(|| "Choose an existing GeoSoilRecord for this IGE".to_owned())?;
                record.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_ige_source_ids)).text())?;
                Ok(GeoCommand::UpsertIge { record })
            }
            (GeoFoundationTab::Boreholes, GeoFoundationDraft::Borehole(mut record)) => {
                let id = BoreholeId::new(self.ui.text_input(cx, ids!(gf_borehole_id)).text()).map_err(|error| format!("BoreholeId: {error}"))?;
                ensure_record_id(&record.id, &id, new_record, "Borehole")?;
                record.id = id;
                record.collar_m = [
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_collar_x)).text(), "Collar X, m")?,
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_collar_y)).text(), "Collar Y, m")?,
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_collar_z)).text(), "Collar Z, m")?,
                ];
                record.collar_vertical_datum = optional_field(self.ui.text_input(cx, ids!(gf_borehole_collar_vertical_datum)).text());
                record.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_borehole_source_ids)).text())?;
                record.depth_datum = self.ui.text_input(cx, ids!(gf_borehole_depth_datum)).text().trim().to_owned();
                record.drilling_date = optional_field(self.ui.text_input(cx, ids!(gf_borehole_date)).text());
                record.crs = optional_field(self.ui.text_input(cx, ids!(gf_borehole_crs)).text());
                record.survey_source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_borehole_survey_sources)).text())?;
                record.evidence_source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_borehole_evidence_sources)).text())?;
                record.total_depth_m = parameter_from_fields(
                    self.ui.drop_down(cx, ids!(gf_borehole_total_depth_state)).selected_item().min(2),
                    &self.ui.text_input(cx, ids!(gf_borehole_total_depth)).text(),
                    &self.ui.text_input(cx, ids!(gf_borehole_total_depth_reason)).text(),
                    &self.ui.text_input(cx, ids!(gf_borehole_total_depth_sources)).text(), "[]", ParameterUnit::Metre, Some(&record.total_depth_m),
                )?;
                if let Some(interval_id) = self.geo_foundation_ui.selected_interval.as_ref() {
                    if let Some(interval) = record.intervals.iter_mut().find(|interval| &interval.id == interval_id) {
                        let changed_id = geo_core::BoreholeIntervalId::new(self.ui.text_input(cx, ids!(gf_interval_id)).text()).map_err(|error| format!("Interval ID: {error}"))?;
                        if changed_id != interval.id { return Err("Stored interval IDs are immutable; add a new interval instead.".into()); }
                        interval.top_depth_m = parse_finite_field(&self.ui.text_input(cx, ids!(gf_interval_top)).text(), "Interval top depth, m")?;
                        interval.bottom_depth_m = parse_finite_field(&self.ui.text_input(cx, ids!(gf_interval_bottom)).text(), "Interval bottom depth, m")?;
                        let ige_index = self.ui.drop_down(cx, ids!(gf_interval_ige)).selected_item();
                        interval.ige_id = self.geo_foundation_ui.ige_ids.get(ige_index).cloned().ok_or_else(|| "Choose an existing IGE for this interval".to_owned())?;
                        interval.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_interval_source_ids)).text())?;
                    }
                }
                Ok(GeoCommand::UpsertBorehole { record })
            }
            (GeoFoundationTab::Model, GeoFoundationDraft::Model(mut record)) => {
                let id = GroundModelId::new(self.ui.text_input(cx, ids!(gf_ground_model_id)).text()).map_err(|error| format!("GroundModelId: {error}"))?;
                ensure_record_id(&record.id, &id, new_record, "GroundModel")?;
                record.id = id;
                record.name = self.ui.text_input(cx, ids!(gf_ground_model_name)).text().trim().to_owned();
                record.frame.name = self.ui.text_input(cx, ids!(gf_frame_name)).text().trim().to_owned();
                record.frame.crs = optional_field(self.ui.text_input(cx, ids!(gf_frame_crs)).text());
                record.frame.vertical_datum = optional_field(self.ui.text_input(cx, ids!(gf_frame_datum)).text());
                record.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_model_source_ids)).text())?;
                record.allow_pinch = self.ui.check_box(cx, ids!(gf_allow_pinch)).active(cx);
                if let Some(triangle_id) = self.geo_foundation_ui.selected_triangle.as_ref() {
                    let Some(triangle) = record.triangles.iter_mut().find(|triangle| &triangle.id == triangle_id) else { return Err("Select an existing triangle to edit; a new triangle requires three distinct model boreholes.".into()); };
                    let changed_id = geo_core::TriangleId::new(self.ui.text_input(cx, ids!(gf_triangle_id)).text()).map_err(|error| format!("TriangleId: {error}"))?;
                    if changed_id != triangle.id { return Err("Stored triangle IDs are immutable.".into()); }
                    triangle.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_triangle_source_ids)).text())?;
                    let a = self.geo_foundation_ui.model_borehole_ids.get(self.ui.drop_down(cx, ids!(gf_triangle_vertex_a)).selected_item()).cloned();
                    let b = self.geo_foundation_ui.model_borehole_ids.get(self.ui.drop_down(cx, ids!(gf_triangle_vertex_b)).selected_item()).cloned();
                    let c = self.geo_foundation_ui.model_borehole_ids.get(self.ui.drop_down(cx, ids!(gf_triangle_vertex_c)).selected_item()).cloned();
                    let vertices = [a.ok_or_else(|| "Triangle vertex A must be a model borehole".to_owned())?, b.ok_or_else(|| "Triangle vertex B must be a model borehole".to_owned())?, c.ok_or_else(|| "Triangle vertex C must be a model borehole".to_owned())?];
                    if vertices[0] == vertices[1] || vertices[1] == vertices[2] || vertices[0] == vertices[2] { return Err("A triangle requires three distinct model boreholes.".into()); }
                    triangle.vertices = vertices;
                }
                Ok(GeoCommand::UpsertModel { record })
            }
            (GeoFoundationTab::Contacts, _) => {
                let mut model = self.geo_foundation_current_model()?;
                let contact_id = geo_core::HorizonId::new(self.ui.text_input(cx, ids!(gf_contact_id)).text()).map_err(|error| format!("HorizonId: {error}"))?;
                let contact_index = model.contacts.iter().position(|contact| contact.id == contact_id);
                if contact_index.is_none() && !new_record { return Err("Select an existing ordered contact before editing it.".into()); }
                let ige_id = self.geo_foundation_ui.ige_ids.get(self.ui.drop_down(cx, ids!(gf_contact_ige)).selected_item()).cloned().ok_or_else(|| "Choose an existing IGE for this contact.".to_owned())?;
                if let Some(index) = contact_index {
                    if let Some(contact) = model.contacts.get(index) {
                        ensure_record_id(&contact.id, &contact_id, new_record, "HorizonId")?;
                    }
                }
                if contact_index.is_none() {
                    model.contacts.push(LayerContact {
                        id: contact_id.clone(), ige_id: ige_id.clone(), elevations_abs_m: BTreeMap::new(),
                        source_ids: Vec::new(), vertical_datum: None,
                    });
                }
                let contact = model.contacts.iter_mut().find(|contact| contact.id == contact_id).ok_or_else(|| "Contact horizon could not be staged.".to_owned())?;
                contact.ige_id = ige_id;
                contact.vertical_datum = optional_field(self.ui.text_input(cx, ids!(gf_contact_vertical_datum)).text());
                contact.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_contact_source_ids)).text())?;
                let borehole_id = self.geo_foundation_ui.model_borehole_ids.get(self.ui.drop_down(cx, ids!(gf_contact_borehole)).selected_item()).cloned().ok_or_else(|| "Choose a borehole in the selected model.".to_owned())?;
                let previous = contact.elevations_abs_m.get(&borehole_id);
                let parameter = parameter_from_fields(
                    self.ui.drop_down(cx, ids!(gf_contact_parameter_state)).selected_item().min(2),
                    &self.ui.text_input(cx, ids!(gf_contact_elevation)).text(),
                    &self.ui.text_input(cx, ids!(gf_contact_missing_reason)).text(),
                    &self.ui.text_input(cx, ids!(gf_contact_source_ids)).text(),
                    &self.ui.text_input(cx, ids!(gf_contact_candidates)).text(), ParameterUnit::Metre, previous,
                )?;
                contact.elevations_abs_m.insert(borehole_id, parameter);
                Ok(GeoCommand::UpsertModel { record: model })
            }
            (GeoFoundationTab::UnknownZones, GeoFoundationDraft::UnknownZone(mut zone)) if new_record => {
                let mut model = self.geo_foundation_current_model()?;
                zone.polygon_xy_m = parse_xy_pairs(&self.ui.text_input(cx, ids!(gf_unknown_polygon)).text(), "Unknown-zone polygon")?;
                zone.reason = self.ui.text_input(cx, ids!(gf_unknown_reason)).text().trim().to_owned();
                zone.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_unknown_source_ids)).text())?;
                model.unknown_zones.push(zone);
                Ok(GeoCommand::UpsertModel { record: model })
            }
            (GeoFoundationTab::UnknownZones, GeoFoundationDraft::UnknownZone(mut zone)) => {
                let mut model = self.geo_foundation_current_model()?;
                let index = self.geo_foundation_ui.unknown_zone_index.ok_or_else(|| "Select an explicit unknown zone to edit.".to_owned())?;
                let stored = model.unknown_zones.get(index).ok_or_else(|| "Selected unknown zone is no longer in the source model.".to_owned())?;
                zone.polygon_xy_m = parse_xy_pairs(&self.ui.text_input(cx, ids!(gf_unknown_polygon)).text(), "Unknown-zone polygon")?;
                zone.reason = self.ui.text_input(cx, ids!(gf_unknown_reason)).text().trim().to_owned();
                zone.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_unknown_source_ids)).text())?;
                if stored != &zone { model.unknown_zones[index] = zone; }
                Ok(GeoCommand::UpsertModel { record: model })
            }
            (GeoFoundationTab::UnknownZones, _) => {
                let mut model = self.geo_foundation_current_model()?;
                let index = self.geo_foundation_ui.unknown_zone_index.ok_or_else(|| "Select an explicit unknown zone to edit.".to_owned())?;
                let zone = model.unknown_zones.get_mut(index).ok_or_else(|| "Selected unknown zone is no longer in the source model.".to_owned())?;
                zone.polygon_xy_m = parse_xy_pairs(&self.ui.text_input(cx, ids!(gf_unknown_polygon)).text(), "Unknown-zone polygon")?;
                zone.reason = self.ui.text_input(cx, ids!(gf_unknown_reason)).text().trim().to_owned();
                zone.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_unknown_source_ids)).text())?;
                Ok(GeoCommand::UpsertModel { record: model })
            }
            (GeoFoundationTab::Water, GeoFoundationDraft::Scenario(mut record)) if !self.geo_foundation_ui.water_editing_observation => {
                let id = GroundScenarioId::new(self.ui.text_input(cx, ids!(gf_scenario_id)).text()).map_err(|error| format!("GroundScenarioId: {error}"))?;
                ensure_record_id(&record.id, &id, new_record, "GroundScenario")?;
                record.id = id;
                record.name = self.ui.text_input(cx, ids!(gf_scenario_name)).text().trim().to_owned();
                record.model_id = self.geo_foundation_ui.model_ids.get(self.ui.drop_down(cx, ids!(gf_scenario_model)).selected_item()).cloned().ok_or_else(|| "Choose the scenario's ground model.".to_owned())?;
                record.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_scenario_source_ids)).text())?;
                record.water = match self.ui.drop_down(cx, ids!(gf_water_kind)).selected_item() {
                    0 => geo_core::WaterScenario::Unknown,
                    1 => {
                        let source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_water_source_ids)).text())?;
                        if source_ids.is_empty() { return Err("ExplicitDry requires explicit source IDs; it is not inferred from missing water observations.".into()); }
                        geo_core::WaterScenario::ExplicitDry { source_ids }
                    }
                    2 => {
                        let ids = parse_id_lines::<geo_core::WaterObservationId>(&self.ui.text_input(cx, ids!(gf_water_observation_ids_text)).text(), "Water observation ID")?;
                        if ids.is_empty() { return Err("Observation-based water scenario requires at least one stored observation ID.".into()); }
                        geo_core::WaterScenario::Observations { observation_ids: ids }
                    }
                    _ => return Err("Unknown water scenario option.".into()),
                };
                Ok(GeoCommand::UpsertScenario { record })
            }
            (GeoFoundationTab::Water, GeoFoundationDraft::WaterObservation(mut record)) if self.geo_foundation_ui.water_editing_observation => {
                let id = geo_core::WaterObservationId::new(self.ui.text_input(cx, ids!(gf_water_observation_id)).text()).map_err(|error| format!("WaterObservationId: {error}"))?;
                ensure_record_id(&record.id, &id, new_record, "WaterObservation")?;
                record.id = id;
                record.xy_m = [
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_water_x)).text(), "Water observation X, m")?,
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_water_y)).text(), "Water observation Y, m")?,
                ];
                record.observed_at = optional_field(self.ui.text_input(cx, ids!(gf_water_observed_at)).text());
                record.elevation_abs_m = match self.ui.drop_down(cx, ids!(gf_water_elevation_state)).selected_item() {
                    0 => Parameter::Missing { reason: "NOT_PROVIDED".into() },
                    1 => {
                        let value = parse_finite_field(&self.ui.text_input(cx, ids!(gf_water_elevation)).text(), "Water elevation, m")?;
                        let source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_water_elevation_source_ids)).text())?;
                        if source_ids.is_empty() { return Err("Known water elevation requires explicit source IDs.".into()); }
                        Parameter::Known { value, source_ids, metadata: ParameterMetadata { unit: ParameterUnit::Metre, confidence_class: ConfidenceClass::Unverified, engineering_verified: false, ..ParameterMetadata::default() } }
                    }
                    2 => return Err("Conflicting water elevations require explicit candidates; this editor does not fabricate them.".into()),
                    _ => return Err("Unknown water elevation status.".into()),
                };
                Ok(GeoCommand::UpsertWaterObservation { record })
            }
            (GeoFoundationTab::Sections, GeoFoundationDraft::Section(mut record)) => {
                let id = GeoSectionId::new(self.ui.text_input(cx, ids!(gf_section_id)).text()).map_err(|error| format!("GeoSectionId: {error}"))?;
                ensure_record_id(&record.id, &id, new_record, "InterpretedSection")?;
                record.id = id;
                record.model_id = self.geo_foundation_ui.model_ids.get(self.ui.drop_down(cx, ids!(gf_section_model)).selected_item()).cloned().ok_or_else(|| "Choose the ground model for this section.".to_owned())?;
                record.polyline_xy_m = parse_xy_pairs(&self.ui.text_input(cx, ids!(gf_section_alignment)).text(), "Section alignment")?;
                record.source_ids = parse_source_ids(&self.ui.text_input(cx, ids!(gf_section_source_ids)).text())?;
                Ok(GeoCommand::UpsertSection { record })
            }
            _ => Err("The selected editor does not have a complete typed source draft. Reload or select a supported record.".into()),
        }
    }

    fn geo_foundation_current_model(&self) -> Result<GroundModel, String> {
        let projection = self.geo_foundation_ui.projection.as_ref().ok_or_else(|| "No current GEO projection.".to_owned())?;
        let id = self.geo_foundation_ui.selected_model.as_ref().ok_or_else(|| "Select a stored ground model.".to_owned())?;
        projection.state.records.models.get(id).cloned().ok_or_else(|| "Selected ground model is unavailable.".to_owned())
    }

    fn geo_foundation_load_demo_ui(&mut self, cx: &mut Cx) {
        if self.document.is_none() {
            self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Open a Document before loading the shared synthetic example.");
            return;
        }
        if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty || self.geo_foundation_ui.layout_fields_dirty
            || self.geo_foundation_ui.analysis_controls_dirty || self.geo_foundation_ui.binding_stale
        {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or explicitly reload all current drafts before replacing this Document with the shared synthetic example.");
            return;
        }
        match self.geo_foundation_load_demo(cx) {
            Ok(()) => {
                self.geo_foundation_ui.draft = None;
                self.geo_foundation_ui.draft_dirty = false;
                self.geo_foundation_ui.new_record = false;
                self.geo_foundation_ui.layout_draft = None;
                self.geo_foundation_ui.layout_dirty = false;
                self.geo_foundation_ui.layout_fields_dirty = false;
                self.geo_foundation_ui.new_placement_ids.clear();
                self.geo_foundation_ui.new_tier_ids.clear();
                self.geo_foundation_ui.layout_link_model = None;
                self.geo_foundation_ui.layout_link_scenario = None;
                self.geo_foundation_ui.selected_model = None;
                self.geo_foundation_ui.selected_scenario = None;
                self.geo_foundation_ui.selected_snapshot = None;
                self.geo_foundation_ui.selected_placement = None;
                self.geo_foundation_ui.selected_tier_id = None;
                self.geo_foundation_ui.selected_foundation_snapshot = None;
                self.geo_foundation_clear_analysis_controls(cx);
                self.geo_foundation_ui.ground_map = None;
                self.geo_foundation_ui.ground_map_key = None;
                self.geo_foundation_ui.ground_map_status = "No midpoint map requested. Sampling is not continuous coverage.".into();
                self.geo_foundation_ui.fit_pending = true;
                self.geo_foundation_refresh_projection(cx);
                self.geo_foundation_render_changed(cx, true);
                self.geo_foundation_ui.status = "SYNTHETIC_EXAMPLE · shared Core fixture loaded into the current Document; no solver ran".into();
                self.geo_foundation_ui.report_text.push_str("\n\nShared synthetic example loaded by explicit user action. No foundation solver was run.");
                self.geo_foundation_update_header(cx);
            }
            Err(error) => self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", &format!("Shared synthetic example was not loaded: {error}")),
        }
        self.geo_foundation_apply_visibility(cx);
    }

    fn geo_foundation_reload(&mut self, cx: &mut Cx) {
        self.geo_foundation_ui.draft_dirty = false;
        self.geo_foundation_ui.layout_dirty = false;
        self.geo_foundation_ui.layout_fields_dirty = false;
        self.geo_foundation_ui.new_placement_ids.clear();
        self.geo_foundation_ui.new_tier_ids.clear();
        self.geo_foundation_ui.new_record = false;
        self.geo_foundation_ui.binding_stale = false;
        self.geo_foundation_clear_analysis_controls(cx);
        self.geo_foundation_ui.ground_map = None;
        self.geo_foundation_ui.ground_map_key = None;
        self.geo_foundation_ui.ground_map_status = "No midpoint map requested. Sampling is not continuous coverage.".into();
        self.geo_foundation_ui.selected_foundation_snapshot = None;
        self.geo_foundation_ui.pan_anchor = None;
        self.geo_foundation_ui.attempt_status = None;
        self.geo_foundation_ui.attempt_message.clear();
        self.geo_foundation_refresh_projection(cx);
        self.geo_foundation_render_changed(cx, false);
        self.geo_foundation_apply_visibility(cx);
    }

    fn geo_foundation_save_snapshot(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty || self.geo_foundation_ui.binding_stale {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload GEO and layout drafts before saving a snapshot.");
            return;
        }
        let binding = match self.geo_foundation_ui.binding.clone() {
            Some(binding) => binding,
            None => { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "No current Document binding is available."); return; }
        };
        let model_id = match self.geo_foundation_ui.selected_model.clone() {
            Some(id) => id,
            None => { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Select an explicit stored ground model before saving a GEO snapshot."); return; }
        };
        let snapshot_id = match geo_core::GeoSnapshotId::new(self.ui.text_input(cx, ids!(gf_snapshot_id)).text()) {
            Ok(id) => id,
            Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &format!("Snapshot ID: {error}")); return; }
        };
        let selection = GeoSnapshotSelection { model_id, scenario_id: self.geo_foundation_ui.selected_scenario.clone() };
        match self.geo_foundation_execute(cx, vec![GeoCommand::SaveSnapshot { id: snapshot_id.clone(), selection }], binding) {
            Ok(()) => {
                self.geo_foundation_ui.selected_snapshot = Some(snapshot_id);
                self.geo_foundation_ui.selected_foundation_snapshot = None;
                self.geo_foundation_refresh_projection(cx);
                self.geo_foundation_render_changed(cx, false);
                self.geo_foundation_ui.status = "INPUT_ONLY · immutable GEO snapshot saved; no analysis was run".into();
                self.geo_foundation_update_header(cx);
            }
            Err(error) => {
                let code = if error.starts_with("STALE_DRAFT") { "STALE_BINDING" } else { "INVALID_INPUT" };
                self.geo_foundation_show_status(cx, code, &error);
            }
        }
    }

    fn geo_foundation_run_foundation_analysis(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty || self.geo_foundation_ui.layout_fields_dirty
            || self.geo_foundation_ui.binding_stale || self.geo_foundation_ui.analysis_controls_stale
        {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply or reload GEO and layout drafts before requesting the conditional foundation analysis.");
            return;
        }
        let Some(projection) = self.geo_foundation_ui.projection.as_ref() else {
            self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "No current GEO/Foundation projection is available.");
            return;
        };
        let Some(layout) = projection.layout.as_ref() else {
            self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Save a FoundationLayout before requesting analysis.");
            return;
        };
        let placement = self.geo_foundation_ui.selected_placement.as_ref()
            .and_then(|id| layout.input.layout.elements.iter().find(|placement| foundation_placement_id(placement) == id));
        let Some(FoundationPlacement::Raft { id, .. }) = placement else {
            self.geo_foundation_show_status(cx, "FOUNDATION_INPUT_HOLD", "The selected placement must be an explicit Raft for this conditional analysis path.");
            return;
        };
        let foundation_id = id.clone();
        let binding = match self.geo_foundation_ui.projection.as_ref().map(|projection| projection.binding.clone()) {
            Some(binding) => binding,
            None => { self.geo_foundation_show_status(cx, "STALE_BINDING", "Reload the Document projection before analysis."); return; }
        };
        let Some(projection) = self.geo_foundation_ui.projection.clone() else { return; };
        let template = self.geo_foundation_ui.selected_control_template.as_ref()
            .and_then(|selected| projection.control_templates.iter().find(|(id, _)| id == selected && id == &foundation_id).map(|(_, input)| input.clone()));
        let input_result = if self.geo_foundation_ui.analysis_uses_configured_reactions {
            let Some(template) = template else {
                self.geo_foundation_show_status(cx, "FOUNDATION_INPUT_HOLD", "Choose an existing numerical control template for the selected Raft before using saved configured reactions.");
                return;
            };
            let Some(source_id) = self.geo_foundation_ui.selected_configured_load_source else {
                self.geo_foundation_show_status(cx, "FOUNDATION_INPUT_HOLD", "Choose a saved configured load source linked to the selected Raft.");
                return;
            };
            let source = projection.configured_load_sources.iter().find(|source| source.snapshot_id == source_id);
            if !source.is_some_and(|source| source.foundation_ids.contains(&foundation_id)) {
                self.geo_foundation_show_status(cx, "FOUNDATION_INPUT_HOLD", "The selected saved load source does not contain the current Raft ID.");
                return;
            }
            let controls = match self.geo_foundation_read_bedding_controls(cx) {
                Ok(controls) => controls,
                Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &error); return; }
            };
            let input = match gf_api::apply_bedding_numerical_controls(&template, controls.0, controls.1, &controls.2, controls.3) {
                Ok(input) => input,
                Err(error) => { self.geo_foundation_show_status(cx, "FOUNDATION_INPUT_HOLD", &error); return; }
            };
            (Some(source_id), input, String::new())
        } else {
            let (input, source_reference) = if let Some(basis) = template {
                if basis.foundation_id != foundation_id {
                    self.geo_foundation_show_status(cx, "INVALID_INPUT", "The selected numerical control template's foundation_id must match the selected Raft.");
                    return;
                }
                let controls = match self.geo_foundation_read_manual_controls(cx) {
                    Ok(controls) => controls,
                    Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &error); return; }
                };
                let source_reference = controls.source_reference.clone();
                let input = match gf_api::apply_manual_numerical_controls(&basis, &controls) {
                    Ok(input) => input,
                    Err(error) => { self.geo_foundation_show_status(cx, "FOUNDATION_INPUT_HOLD", &error); return; }
                };
                (input, source_reference)
            } else {
                let raw = self.ui.text_input(cx, ids!(gf_foundation_input_json)).text();
                if raw.trim().is_empty() {
                    self.geo_foundation_show_status(cx, "FOUNDATION_INPUT_HOLD", "Choose an existing numerical control template or provide a complete advanced RigidMatInput JSON.");
                    return;
                }
                let input = match serde_json::from_str::<RigidMatInput>(&raw) {
                    Ok(input) => input,
                    Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &format!("Advanced RigidMatInput JSON: {error}")); return; }
                };
                if input.foundation_id != foundation_id {
                    self.geo_foundation_show_status(cx, "INVALID_INPUT", "Advanced RigidMatInput foundation_id must match the selected Raft.");
                    return;
                }
                let source_reference = self.ui.text_input(cx, ids!(gf_foundation_source_reference)).text().trim().to_owned();
                if source_reference.is_empty() {
                    self.geo_foundation_show_status(cx, "INVALID_INPUT", "Advanced JSON mode still requires an explicit source/assumption reference.");
                    return;
                }
                (input, source_reference)
            };
            (None, input, source_reference)
        };
        let prior_snapshot_ids = self.geo_foundation_ui.foundation_snapshot_ids.clone();
        self.geo_foundation_clear_scene(cx);
        self.ui.label(cx, ids!(gf_attempt_status)).set_text(cx, "ANALYSIS_ATTEMPT · source and Core readiness checks are pending");
        let request_result = if let Some(snapshot_id) = input_result.0 {
            self.geo_foundation_analyze_from_configured(cx, foundation_id.as_str(), snapshot_id, input_result.1, binding)
        } else {
            self.geo_foundation_analyze(cx, foundation_id.as_str(), input_result.1, &input_result.2, binding)
        };
        match request_result {
            Ok(()) => {
                self.geo_foundation_ui.analysis_controls_dirty = false;
                self.geo_foundation_ui.analysis_controls_stale = false;
                self.geo_foundation_refresh_projection(cx);
                self.geo_foundation_ui.selected_foundation_snapshot = self.geo_foundation_ui.projection.as_ref()
                    .and_then(|projection| projection.foundation_snapshots.iter()
                        .find(|(id, snapshot, status)| !prior_snapshot_ids.contains(id)
                            && snapshot.input.effective_input.foundation_id == foundation_id
                            && *status == ai_revit_core::EngineeringSnapshotStatus::Current)
                        .map(|(id, _, _)| *id));
                self.geo_foundation_sync_foundation_selectors(cx);
                self.geo_foundation_ui.analysis_controls_binding = self.geo_foundation_ui.binding.clone();
                self.geo_foundation_ui.analysis_controls_stale = false;
                self.geo_foundation_ui.status = "CONDITIONAL_RESULT · Foundation snapshot saved; engineering acceptance remains HOLD".into();
                self.geo_foundation_ui.attempt_status = Some("CONDITIONAL_RESULT".into());
                self.geo_foundation_ui.attempt_message = "Conditional numerical pressure/settlement only; inspect source-matched snapshot. Bearing, design and normative acceptance remain HOLD.".into();
                self.geo_foundation_update_header(cx);
                self.geo_foundation_render_changed(cx, false);
            }
            Err(error) => {
                let code = if error.starts_with("STALE_DRAFT") || error.starts_with("SOURCE_BINDING_HOLD") { "SOURCE_BINDING_HOLD" }
                    else if error.starts_with("FOUNDATION_INPUT_HOLD") || error.starts_with("FOUNDATION_LOAD_SOURCE_HOLD") || error.starts_with("MODEL_INCOMPLETE") { "FOUNDATION_INPUT_HOLD" }
                    else if error.starts_with("FOUNDATION_ANALYSIS_ATTEMPT_FAILED") { "ANALYSIS_ATTEMPT_FAILED" }
                    else { "ANALYSIS_REQUEST_FAILED" };
                self.geo_foundation_ui.selected_foundation_snapshot = None;
                self.geo_foundation_clear_scene(cx);
                self.geo_foundation_show_status(cx, code, &error);
                self.ui.text_input(cx, ids!(gf_report_text)).set_text(cx, &format!("{}\n\nLatest explicit Analyze attempt: {code} · {error}\nNo current pressure or settlement layer is displayed. Bearing, code settlement, reinforcement and ULS/SLS acceptance remain HOLD.", self.geo_foundation_ui.report_text));
            }
        }
        self.geo_foundation_apply_visibility(cx);
    }

    fn geo_foundation_read_bedding_controls(&self, cx: &Cx) -> Result<(f64, f64, String, bool), String> {
        let k = parse_finite_field(&self.ui.text_input(cx, ids!(gf_manual_k_kn_m3)).text(), "Manual bedding k, kN/m³")?;
        let gap = parse_finite_field(&self.ui.text_input(cx, ids!(gf_manual_gap_m)).text(), "Manual bedding gap, m")?;
        let note = self.ui.text_input(cx, ids!(gf_foundation_bedding_note)).text().trim().to_owned();
        if note.is_empty() { return Err("A specific manual bedding-assumption note is required; Geo E/ν is not converted to k.".into()); }
        let include_weight = self.ui.check_box(cx, ids!(gf_manual_own_weight)).active(cx);
        Ok((k, gap, note, include_weight))
    }

    fn geo_foundation_read_manual_controls(&self, cx: &Cx) -> Result<gf_api::GfManualNumericalControls, String> {
        let (bedding_k_kn_m3, gap_m, bedding_note, include_raft_own_weight) = self.geo_foundation_read_bedding_controls(cx)?;
        Ok(gf_api::GfManualNumericalControls {
            n_kn: parse_finite_field(&self.ui.text_input(cx, ids!(gf_manual_n_kn)).text(), "Manual vertical resultant N, kN")?,
            physical_mx_knm: parse_finite_field(&self.ui.text_input(cx, ids!(gf_manual_mx_knm)).text(), "Manual physical Mx, kN·m")?,
            physical_my_knm: parse_finite_field(&self.ui.text_input(cx, ids!(gf_manual_my_knm)).text(), "Manual physical My, kN·m")?,
            bedding_k_kn_m3,
            gap_m,
            include_raft_own_weight,
            source_reference: self.ui.text_input(cx, ids!(gf_foundation_source_reference)).text().trim().to_owned(),
            bedding_note,
        })
    }

    fn geo_foundation_run_query(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.binding_stale {
            self.geo_foundation_ui.query_text = "STALE_BINDING · query suppressed while a source record draft is unapplied or stale.".into();
            self.geo_foundation_ui.query_result_binding = None;
            self.geo_foundation_update_header(cx);
            return;
        }
        let Some(resolved) = self.geo_foundation_ui.projection.as_ref().and_then(|projection| projection.resolved.as_ref()) else {
            self.geo_foundation_ui.query_text = "MODEL_INCOMPLETE · select a source-bound model and explicit water scenario. Unknown is not dry.".into();
            self.geo_foundation_ui.query_result_binding = self.geo_foundation_ui.projection.as_ref().map(geo_foundation_query_identity);
            self.geo_foundation_update_header(cx);
            return;
        };
        let result = match self.ui.drop_down(cx, ids!(gf_query_kind)).selected_item() {
            0 => {
                let x = parse_finite_field(&self.ui.text_input(cx, ids!(gf_query_x)).text(), "Query X, m");
                let y = parse_finite_field(&self.ui.text_input(cx, ids!(gf_query_y)).text(), "Query Y, m");
                x.and_then(|x| y.map(|y| (x, y))).and_then(|(x,y)| geo_core::geometry::query_column(resolved, [x,y]).map_err(|error| error.to_string()))
            }
            1 => {
                let section_id = self.geo_foundation_ui.selected_section.as_ref().ok_or_else(|| "Select an interpreted section.".to_owned());
                let spacing = parse_finite_field(&self.ui.text_input(cx, ids!(gf_query_spacing)).text(), "Section query spacing, m");
                section_id.and_then(|section_id| spacing.map(|spacing| (section_id, spacing))).and_then(|(section_id, spacing)| {
                    if spacing <= 0.0 { return Err("Section query spacing must be greater than 0.".into()); }
                    let section = resolved.snapshot.records.sections.get(section_id).ok_or_else(|| "Selected section is not inside the resolved snapshot closure.".to_owned())?;
                    geo_core::geometry::query_section(resolved, &section.polyline_xy_m, spacing).map_err(|error| error.to_string())
                })
            }
            2 => geo_core::geometry::layer_volumes(resolved).map_err(|error| error.to_string()),
            _ => Err("Unknown GEO query kind.".into()),
        };
        self.geo_foundation_ui.query_text = match result {
            Ok(result) => {
                let provenance = self.geo_foundation_ui.projection.as_ref().and_then(|projection| projection.selected_snapshot.as_ref())
                    .map(|snapshot| format!("\nSource closure {} · operation {} · {:?} · {}", snapshot.content_fingerprint, snapshot.source_revision.operation_id,
                        self.geo_foundation_ui.projection.as_ref().and_then(|projection| projection.snapshot_currentness),
                        if self.geo_foundation_ui.projection.as_ref().is_some_and(|projection| projection.snapshot_is_stored) { "stored snapshot" } else { "read-only current-records projection" }))
                    .unwrap_or_default();
                format!("DERIVED GEO QUERY · not solver acceptance\n{}{}", serde_json::to_string_pretty(&result).unwrap_or_else(|error| error.to_string()), provenance)
            }
            Err(error) => format!("MODEL_INCOMPLETE / QUERY HOLD · {error}"),
        };
        self.geo_foundation_ui.query_result_binding = self.geo_foundation_ui.projection.as_ref().map(geo_foundation_query_identity);
        self.geo_foundation_update_header(cx);
    }

    fn geo_foundation_build_ground_map(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty || self.geo_foundation_ui.layout_fields_dirty
            || self.geo_foundation_ui.binding_stale || self.geo_foundation_ui.analysis_controls_stale
        {
            self.geo_foundation_ui.ground_map = None;
            self.geo_foundation_ui.ground_map_key = None;
            self.geo_foundation_ui.ground_map_status = "STALE_SOURCE · apply/reload GEO and FoundationLayout drafts before requesting midpoint samples.".into();
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
            self.geo_foundation_show_status(cx, "STALE_SOURCE", &self.geo_foundation_ui.ground_map_status.clone());
            self.geo_foundation_render_changed(cx, false);
            return;
        }
        let Some(projection) = self.geo_foundation_ui.projection.clone() else {
            self.geo_foundation_ui.ground_map_status = "MODEL_INCOMPLETE · no current Document projection.".into();
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
            return;
        };
        let Some(layout) = projection.layout.as_ref() else {
            self.geo_foundation_ui.ground_map_status = "MODEL_INCOMPLETE · no saved FoundationLayout.".into();
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
            return;
        };
        let Some(foundation_id) = self.geo_foundation_ui.selected_placement.as_ref() else {
            self.geo_foundation_ui.ground_map_status = "MODEL_INCOMPLETE · select an explicit Raft placement.".into();
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
            return;
        };
        if !layout.input.layout.elements.iter().any(|placement| {
            foundation_placement_id(placement) == foundation_id && matches!(placement, FoundationPlacement::Raft { .. })
        }) {
            self.geo_foundation_ui.ground_map_status = "FOUNDATION_INPUT_HOLD · midpoint ground map currently supports an axis-aligned rectangular Raft only.".into();
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
            return;
        }
        let Some(link) = layout.input.ground_links.get(foundation_id.as_str()) else {
            self.geo_foundation_ui.ground_map = None;
            self.geo_foundation_ui.ground_map_key = None;
            self.geo_foundation_ui.ground_map_status = "MODEL_INCOMPLETE · this Raft has no explicit GEO model/scenario link.".into();
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
            return;
        };
        let Some(snapshot) = projection.selected_snapshot.as_ref() else {
            self.geo_foundation_ui.ground_map_status = "MODEL_INCOMPLETE · select the same source-bound model and explicit water scenario as the Raft link.".into();
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
            return;
        };
        if projection.snapshot_currentness != Some(geo_core::SnapshotCurrentness::Current)
            || snapshot.selection != *link
            || projection.resolved.is_none()
        {
            self.geo_foundation_ui.ground_map = None;
            self.geo_foundation_ui.ground_map_key = None;
            self.geo_foundation_ui.ground_map_status = "SOURCE_BINDING_HOLD · selected GEO model/scenario must match the placement link and resolve as current; water Unknown is not Dry.".into();
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
            return;
        }
        let placement_id = foundation_id.clone();
        match self.geo_foundation_ground_map(placement_id.as_str(), 40, 48) {
            Ok(map) => {
                let key = format!("{}|{}|{}|{}|{}|{}",
                    placement_id, layout.input_fingerprint, map.ground.selection_fingerprint,
                    map.ground.content_fingerprint, map.ground.source_revision.operation_id,
                    projection.snapshot_is_stored);
                let area_by_ige = map.midpoint_area_estimate_by_ige_m2.iter()
                    .map(|(ige_id, area)| format!("{}: {:.3} m²", ige_id, area)).collect::<Vec<_>>().join("; ");
                self.geo_foundation_ui.ground_map_status = format!(
                    "SOURCE_BOUND_MIDPOINT_SAMPLES · {} × {} cells · {}\nUnknown/interface sample area {:.3} m². Area-by-IGE totals are midpoint estimates only. No continuous coverage proof, bearing, settlement, water-pressure or E-to-k inference.\n{}",
                    map.nx, map.ny, if projection.snapshot_is_stored { "selected GEO snapshot is stored and current" } else { "selected GEO projection is read-only current records" },
                    map.unknown_or_interface_sample_area_m2, area_by_ige);
                self.geo_foundation_ui.ground_map_key = Some(key);
                self.geo_foundation_ui.ground_map = Some(map);
                self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
                self.geo_foundation_render_changed(cx, false);
            }
            Err(error) => {
                self.geo_foundation_ui.ground_map = None;
                self.geo_foundation_ui.ground_map_key = None;
                self.geo_foundation_ui.ground_map_status = format!("FOUNDATION_MAP_HOLD · {error}\nNo continuous coverage or engineering acceptance is inferred.");
                self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
                self.geo_foundation_render_changed(cx, false);
            }
        }
    }

    fn geo_foundation_save_layout(&mut self, cx: &mut Cx) {
        let Some(binding) = self.geo_foundation_ui.layout_binding.clone().or_else(|| self.geo_foundation_ui.binding.clone()) else {
            self.geo_foundation_show_status(cx, "STALE_BINDING", "Reload the Document before saving a FoundationLayout.");
            return;
        };
        if self.geo_foundation_ui.binding_stale || self.geo_foundation_ui.analysis_controls_stale { self.geo_foundation_show_status(cx, "STALE_BINDING", "The Document changed while GEO/Foundation inputs were edited. Reload before saving."); return; }
        if self.geo_foundation_ui.analysis_controls_dirty && self.geo_foundation_ui.analysis_payload_dirty {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "The load/moment input is still an unapplied draft. Analyze or reload it before changing the FoundationLayout; the text is retained.");
            return;
        }
        if !self.geo_foundation_capture_layout_fields(cx) { return; }
        let input = match self.geo_foundation_read_layout_form(cx) {
            Ok(input) => input,
            Err(error) => { self.geo_foundation_ui.layout_dirty = true; self.geo_foundation_show_status(cx, "INVALID_INPUT", &error); return; }
        };
        self.geo_foundation_clear_scene(cx);
        match self.geo_foundation_replace_layout(cx, input, binding) {
            Ok(()) => {
                self.geo_foundation_ui.layout_dirty = false;
                self.geo_foundation_ui.layout_fields_dirty = false;
                self.geo_foundation_ui.bedding_fields_dirty = false;
                self.geo_foundation_ui.bedding_clear_requested = None;
                self.geo_foundation_ui.new_placement_ids.clear();
                self.geo_foundation_ui.new_tier_ids.clear();
                self.geo_foundation_ui.selected_foundation_snapshot = None;
                self.geo_foundation_ui.layout_draft = None;
                self.geo_foundation_refresh_projection(cx);
                if self.geo_foundation_ui.analysis_controls_dirty && !self.geo_foundation_ui.analysis_payload_dirty {
                    self.geo_foundation_ui.analysis_controls_binding = self.geo_foundation_ui.binding.clone();
                    self.geo_foundation_ui.analysis_controls_stale = false;
                }
                self.geo_foundation_render_changed(cx, false);
            }
            Err(error) => {
                let code = if error.starts_with("STALE_DRAFT") { "STALE_BINDING" } else { "INVALID_INPUT" };
                self.geo_foundation_ui.layout_dirty = true;
                self.geo_foundation_show_status(cx, code, &error);
            }
        }
    }

    fn geo_foundation_add_placement(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.analysis_controls_dirty || self.geo_foundation_ui.analysis_controls_stale { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Apply/reload the GEO and numerical-control drafts before changing the layout."); return; }
        if !self.geo_foundation_capture_layout_fields(cx) { return; }
        let mut layout = self.geo_foundation_ui.layout_draft.clone().unwrap_or_else(|| FoundationLayout {
            id: self.ui.text_input(cx, ids!(gf_layout_id)).text().trim().to_owned(), elements: Vec::new(),
        });
        if layout.id.trim().is_empty() { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Enter a valid FoundationLayout ID before adding a placement."); return; }
        let id_text = GeoFoundationUi::draft_id("foundation", self.geo_foundation_ui.placement_ids.iter().map(|id| id.as_str()));
        let id = match FoundationId::new(id_text) { Ok(id) => id, Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &error.to_string()); return; } };
        let placement = match self.ui.drop_down(cx, ids!(gf_placement_kind)).selected_item() {
            0 => FoundationPlacement::Raft { id: id.clone(), center_m: [0.0; 3], length_x_m: 0.0, width_y_m: 0.0, thickness_m: 0.0 },
            1 => FoundationPlacement::Strip { id: id.clone(), center_m: [0.0; 3], length_m: 0.0, width_m: 0.0, rotation_rad: 0.0, thickness_m: 0.0 },
            2 => FoundationPlacement::Isolated { id: id.clone(), center_m: [0.0; 3], length_x_m: 0.0, width_y_m: 0.0, thickness_m: 0.0 },
            _ => FoundationPlacement::Stepped { id: id.clone(), tiers: Vec::new() },
        };
        layout.elements.push(placement);
        self.geo_foundation_ui.layout_draft = Some(layout);
        self.geo_foundation_ui.selected_placement = Some(id.clone());
        self.geo_foundation_ui.new_placement_ids.insert(id.clone());
        self.geo_foundation_ui.selected_foundation = Some(id.to_string());
        self.geo_foundation_ui.selected_tier_id = None;
        self.geo_foundation_mark_layout_dirty(cx);
        self.geo_foundation_sync_foundation_selectors(cx);
        for widget in [ids!(gf_placement_x), ids!(gf_placement_y), ids!(gf_placement_z), ids!(gf_placement_length_x), ids!(gf_placement_width_y), ids!(gf_placement_thickness), ids!(gf_placement_strip_length), ids!(gf_placement_strip_width), ids!(gf_placement_rotation)] {
            self.geo_foundation_set_input(cx, widget, "");
        }
        self.geo_foundation_set_input(cx, ids!(gf_placement_id), id.as_str());
    }

    fn geo_foundation_delete_placement(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.analysis_controls_dirty || self.geo_foundation_ui.analysis_controls_stale { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Analyze or reload the numerical-control draft before changing the layout."); return; }
        if !self.geo_foundation_capture_layout_fields(cx) { return; }
        let Some(id) = self.geo_foundation_ui.selected_placement.clone() else { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Select a placement to remove from this layout draft."); return; };
        let Some(layout) = self.geo_foundation_ui.layout_draft.as_mut() else { return; };
        let removed_tier_ids: Vec<_> = layout.elements.iter().find(|placement| foundation_placement_id(placement) == &id)
            .and_then(|placement| match placement { FoundationPlacement::Stepped { tiers, .. } => Some(tiers.iter().map(|tier| tier.id.clone()).collect()), _ => None })
            .unwrap_or_default();
        layout.elements.retain(|placement| foundation_placement_id(placement) != &id);
        self.geo_foundation_ui.new_placement_ids.remove(&id);
        for tier_id in removed_tier_ids { self.geo_foundation_ui.new_tier_ids.remove(&tier_id); }
        self.geo_foundation_ui.selected_placement = layout.elements.first().map(|placement| foundation_placement_id(placement).clone());
        self.geo_foundation_ui.selected_foundation = self.geo_foundation_ui.selected_placement.as_ref().map(ToString::to_string);
        self.geo_foundation_ui.selected_tier_id = None;
        self.geo_foundation_mark_layout_dirty(cx);
        self.geo_foundation_sync_foundation_selectors(cx);
    }

    fn geo_foundation_add_tier(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.analysis_controls_dirty || self.geo_foundation_ui.analysis_controls_stale { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Analyze or reload the numerical-control draft before changing the layout."); return; }
        if !self.geo_foundation_capture_layout_fields(cx) { return; }
        let Some(layout) = self.geo_foundation_ui.layout_draft.as_mut() else { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Add a Stepped placement before adding tiers."); return; };
        let Some(id) = self.geo_foundation_ui.selected_placement.as_ref() else { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Select a Stepped placement."); return; };
        let Some(placement) = layout.elements.iter_mut().find(|placement| foundation_placement_id(placement) == id) else { return; };
        let FoundationPlacement::Stepped { tiers, .. } = placement else {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Choose Stepped in the placement kind before adding tiers."); return;
        };
        let tier_id = GeoFoundationUi::draft_id("tier", self.geo_foundation_ui.tier_ids.iter().map(String::as_str));
        tiers.push(FoundationTier { id: tier_id.clone(), center_m: [0.0; 3], length_x_m: 0.0, width_y_m: 0.0, thickness_m: 0.0 });
        self.geo_foundation_ui.new_tier_ids.insert(tier_id.clone());
        self.geo_foundation_ui.selected_tier_id = Some(tier_id.clone());
        self.geo_foundation_mark_layout_dirty(cx);
        self.geo_foundation_sync_foundation_selectors(cx);
        for widget in [ids!(gf_tier_id), ids!(gf_tier_x), ids!(gf_tier_y), ids!(gf_tier_z), ids!(gf_tier_length_x), ids!(gf_tier_width_y), ids!(gf_tier_thickness)] { self.geo_foundation_set_input(cx, widget, ""); }
        self.geo_foundation_set_input(cx, ids!(gf_tier_id), &tier_id);
    }

    fn geo_foundation_delete_tier(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.analysis_controls_dirty || self.geo_foundation_ui.analysis_controls_stale { self.geo_foundation_show_status(cx, "INVALID_INPUT", "Analyze or reload the numerical-control draft before changing the layout."); return; }
        if !self.geo_foundation_capture_layout_fields(cx) { return; }
        let (Some(layout), Some(placement_id), Some(tier_id)) = (self.geo_foundation_ui.layout_draft.as_mut(), self.geo_foundation_ui.selected_placement.clone(), self.geo_foundation_ui.selected_tier_id.clone()) else { self.geo_foundation_show_status(cx, "MODEL_INCOMPLETE", "Select a tier in a Stepped placement."); return; };
        let Some(FoundationPlacement::Stepped { tiers, .. }) = layout.elements.iter_mut().find(|placement| foundation_placement_id(placement) == &placement_id) else { return; };
        tiers.retain(|tier| tier.id != tier_id);
        self.geo_foundation_ui.new_tier_ids.remove(&tier_id);
        self.geo_foundation_ui.selected_tier_id = tiers.first().map(|tier| tier.id.clone());
        self.geo_foundation_mark_layout_dirty(cx);
        self.geo_foundation_sync_foundation_selectors(cx);
    }

    fn geo_foundation_apply_ground_link(&mut self, cx: &mut Cx) {
        if self.geo_foundation_ui.analysis_controls_dirty || self.geo_foundation_ui.analysis_controls_stale {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Analyze or reload the current numerical-control draft before changing a foundation GEO link.");
            return;
        }
        let Some(placement) = self.geo_foundation_ui.selected_placement.clone() else {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "Select a placement before applying or clearing a GEO link.");
            return;
        };
        if !self.geo_foundation_capture_layout_fields(cx) { return; }
        let input = match self.geo_foundation_read_layout_form(cx) {
            Ok(input) => input,
            Err(error) => { self.geo_foundation_show_status(cx, "INVALID_INPUT", &error); return; }
        };
        if !input.layout.elements.iter().any(|value| foundation_placement_id(value) == &placement) {
            self.geo_foundation_show_status(cx, "INVALID_INPUT", "The selected placement is not in the layout draft.");
            return;
        }
        self.geo_foundation_ui.layout_draft = Some(input.layout);
        let link_status = match (&self.geo_foundation_ui.layout_link_model, &self.geo_foundation_ui.layout_link_scenario) {
            (Some(model_id), Some(scenario_id)) => format!("Layout draft · placement {placement} linked to model {model_id} and explicit scenario {scenario_id}."),
            (None, None) => format!("Layout draft · GEO link cleared for placement {placement}."),
            _ => {
                self.geo_foundation_show_status(cx, "INVALID_INPUT", "Choose both a stored model and explicit water scenario, or choose neither to clear this GEO link.");
                return;
            }
        };
        self.geo_foundation_ui.ground_map = None;
        self.geo_foundation_ui.ground_map_key = None;
        self.geo_foundation_ui.ground_map_status = "GEO link changed · request a new source-bound midpoint map explicitly.".into();
        self.geo_foundation_mark_layout_dirty(cx);
        self.ui.text_input(cx, ids!(gf_layout_status)).set_text(cx, &format!("{link_status} Save layout to persist this typed link."));
    }

    fn geo_foundation_read_layout_form(&self, cx: &Cx) -> Result<ai_revit_core::GeoFoundationLayoutInput, String> {
        let projection = self.geo_foundation_ui.projection.as_ref().ok_or_else(|| "No current Document projection for the layout draft.".to_owned())?;
        let mut layout = self.geo_foundation_ui.layout_draft.clone().or_else(|| projection.layout.as_ref().map(|state| state.input.layout.clone()))
            .ok_or_else(|| "Add a typed Foundation placement before saving a layout.".to_owned())?;
        let new_layout_id = self.ui.text_input(cx, ids!(gf_layout_id)).text().trim().to_owned();
        if new_layout_id.is_empty() { return Err("FoundationLayout ID is required.".into()); }
        if projection.layout.as_ref().is_some_and(|stored| stored.input.layout.id != new_layout_id) {
            return Err("Stored FoundationLayout IDs are immutable.".into());
        }
        layout.id = new_layout_id;
        if let Some(placement_id) = self.geo_foundation_ui.selected_placement.as_ref() {
            let element = layout.elements.iter_mut().find(|placement| foundation_placement_id(placement) == placement_id)
                .ok_or_else(|| "Selected placement is not in the layout draft.".to_owned())?;
            let id_text = self.ui.text_input(cx, ids!(gf_placement_id)).text();
            let id = FoundationId::new(id_text).map_err(|error| format!("FoundationId: {error}"))?;
            let original_exists = projection.layout.as_ref().is_some_and(|stored| stored.input.layout.elements.iter().any(|placement| foundation_placement_id(placement) == placement_id));
            if original_exists && &id != placement_id { return Err("Stored FoundationId values are immutable.".into()); }
            let placement_id_for_lookup = id.clone();
            let kind = self.ui.drop_down(cx, ids!(gf_placement_kind)).selected_item();
            let prior_tiers = match element { FoundationPlacement::Stepped { tiers, .. } => tiers.clone(), _ => Vec::new() };
            *element = match kind {
                0 => FoundationPlacement::Raft { id: id.clone(), center_m: [
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_x)).text(), "Raft center X, m")?,
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_y)).text(), "Raft center Y, m")?,
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_z)).text(), "Raft lower-face Z, m")?,
                ], length_x_m: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_length_x)).text(), "Raft length X, m")?, width_y_m: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_width_y)).text(), "Raft width Y, m")?, thickness_m: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_thickness)).text(), "Raft thickness, m")? },
                1 => FoundationPlacement::Strip { id: id.clone(), center_m: [
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_x)).text(), "Strip center X, m")?,
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_y)).text(), "Strip center Y, m")?,
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_z)).text(), "Strip lower-face Z, m")?,
                ], length_m: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_strip_length)).text(), "Strip length, m")?, width_m: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_strip_width)).text(), "Strip width, m")?, rotation_rad: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_rotation)).text(), "Strip rotation, rad")?, thickness_m: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_thickness)).text(), "Strip thickness, m")? },
                2 => FoundationPlacement::Isolated { id: id.clone(), center_m: [
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_x)).text(), "Isolated center X, m")?,
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_y)).text(), "Isolated center Y, m")?,
                    parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_z)).text(), "Isolated lower-face Z, m")?,
                ], length_x_m: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_length_x)).text(), "Isolated foundation length X, m")?, width_y_m: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_width_y)).text(), "Isolated foundation width Y, m")?, thickness_m: parse_finite_field(&self.ui.text_input(cx, ids!(gf_placement_thickness)).text(), "Isolated foundation thickness, m")? },
                _ => FoundationPlacement::Stepped { id: id.clone(), tiers: prior_tiers },
            };
            if let Some(FoundationPlacement::Stepped { tiers, .. }) = layout.elements.iter_mut().find(|placement| foundation_placement_id(placement) == &placement_id_for_lookup) {
                if let Some(tier_id) = self.geo_foundation_ui.selected_tier_id.as_ref() {
                    let tier = tiers.iter_mut().find(|tier| &tier.id == tier_id).ok_or_else(|| "Selected tier is not in this Stepped placement.".to_owned())?;
                    let next_tier_id = self.ui.text_input(cx, ids!(gf_tier_id)).text();
                    if next_tier_id != tier.id && projection.layout.as_ref().is_some_and(|stored| stored.input.layout.elements.iter().any(|placement| matches!(placement, FoundationPlacement::Stepped { tiers, .. } if tiers.iter().any(|stored| stored.id == tier.id)))) {
                        return Err("Stored tier IDs are immutable.".into());
                    }
                    tier.id = next_tier_id;
                    tier.center_m = [
                        parse_finite_field(&self.ui.text_input(cx, ids!(gf_tier_x)).text(), "Tier center X, m")?,
                        parse_finite_field(&self.ui.text_input(cx, ids!(gf_tier_y)).text(), "Tier center Y, m")?,
                        parse_finite_field(&self.ui.text_input(cx, ids!(gf_tier_z)).text(), "Tier lower-face Z, m")?,
                    ];
                    tier.length_x_m = parse_finite_field(&self.ui.text_input(cx, ids!(gf_tier_length_x)).text(), "Tier length X, m")?;
                    tier.width_y_m = parse_finite_field(&self.ui.text_input(cx, ids!(gf_tier_width_y)).text(), "Tier width Y, m")?;
                    tier.thickness_m = parse_finite_field(&self.ui.text_input(cx, ids!(gf_tier_thickness)).text(), "Tier thickness, m")?;
                }
            }
        }
        let mut ground_links = projection.layout.as_ref().map(|state| state.input.ground_links.clone()).unwrap_or_default();
        let mut bedding_assumptions = projection.layout.as_ref().map(|state| state.input.bedding_assumptions.clone()).unwrap_or_default();
        if let Some(placement_id) = self.geo_foundation_ui.selected_placement.as_ref() {
            match (&self.geo_foundation_ui.layout_link_model, &self.geo_foundation_ui.layout_link_scenario) {
                (Some(model_id), Some(scenario_id)) => {
                    ground_links.insert(placement_id.to_string(), GeoSnapshotSelection { model_id: model_id.clone(), scenario_id: Some(scenario_id.clone()) });
                }
                (None, None) => { ground_links.remove(placement_id.as_str()); }
                _ => return Err("Choose both a stored ground model and explicit water scenario, or choose neither to clear the link.".into()),
            }
            if self.geo_foundation_ui.bedding_clear_requested.as_ref() == Some(placement_id) {
                bedding_assumptions.remove(placement_id.as_str());
            } else {
                let k_text = self.ui.text_input(cx, ids!(gf_manual_k_kn_m3)).text().trim().to_owned();
                let note = self.ui.text_input(cx, ids!(gf_foundation_bedding_note)).text().trim().to_owned();
                let has_bedding_fields = !k_text.is_empty() || !note.is_empty() || !self.geo_foundation_ui.bedding_source_ids.is_empty();
                if has_bedding_fields {
                    let k_kn_m3 = parse_finite_field(&k_text, "Authored manual bedding k, kN/m³")?;
                    if note.is_empty() { return Err("An explicit authored bedding assumption note is required.".into()); }
                    if self.geo_foundation_ui.bedding_source_ids.is_empty() { return Err("Select at least one GEO SourceRecord from the current linked snapshot closure for the bedding declaration.".into()); }
                    let selected_snapshot = projection.selected_snapshot.as_ref().ok_or_else(|| "Select the linked GEO model/scenario closure before authoring a bedding source.".to_owned())?;
                    for source_id in &self.geo_foundation_ui.bedding_source_ids {
                        if !selected_snapshot.records.sources.contains_key(source_id) {
                            return Err(format!("Bedding source {source_id} is not in the currently selected Geo snapshot closure."));
                        }
                    }
                    bedding_assumptions.insert(placement_id.to_string(), foundation_core::geo_context::FoundationBeddingAssumption::ManualSyntheticUniform {
                        k_kn_m3, assumption_note: note, source_ids: self.geo_foundation_ui.bedding_source_ids.clone(),
                    });
                }
            }
        }
        Ok(ai_revit_core::GeoFoundationLayoutInput { layout, ground_links, bedding_assumptions })
    }

    fn geo_foundation_render_options(&self) -> gf_render::GeoRenderOptions {
        gf_render::GeoRenderOptions {
            view: self.geo_foundation_ui.view,
            show_terrain: self.geo_foundation_ui.show_terrain,
            show_contacts: self.geo_foundation_ui.show_contacts,
            show_boreholes: self.geo_foundation_ui.show_boreholes,
            visible_ige_ids: self.geo_foundation_ui.visible_ige_filter.iter().cloned().collect(),
            show_unknown_zones: self.geo_foundation_ui.show_unknown_zones,
            show_water: self.geo_foundation_ui.show_water,
            show_foundations: self.geo_foundation_ui.show_foundations,
            selected_section: self.geo_foundation_ui.selected_section.clone(),
            section_max_spacing_m: self.geo_foundation_ui.section_max_spacing_m,
        }
    }

    fn geo_foundation_render_context(&self) -> Result<(gf_api::GfProjection, gf_render::GeoRenderOptions, Option<(ai_revit_core::EngineeringSnapshotId, ai_revit_core::GeoFoundationAnalysisSnapshot, ai_revit_core::EngineeringSnapshotStatus)>), String> {
        if self.geo_foundation_ui.draft_dirty || self.geo_foundation_ui.layout_dirty || self.geo_foundation_ui.binding_stale {
            return Err("STALE_SOURCE · unapplied or stale GEO/Foundation draft suppresses rendered results".into());
        }
        let projection = self.geo_foundation_ui.projection.clone().ok_or_else(|| "MODEL_INCOMPLETE · no current GEO projection".to_owned())?;
        if projection.resolved.is_none() {
            return Err("MODEL_INCOMPLETE · select a source-bound model and explicit water scenario; Unknown is not Dry".into());
        }
        let selected = self.geo_foundation_ui.selected_foundation_snapshot.and_then(|id| {
            projection.foundation_snapshots.iter().find(|(snapshot_id, _, _)| *snapshot_id == id)
                .map(|(snapshot_id, snapshot, status)| (*snapshot_id, snapshot.clone(), *status))
        });
        Ok((projection, self.geo_foundation_render_options(), selected))
    }

    fn geo_foundation_render_key(
        &self,
        projection: &gf_api::GfProjection,
        options: &gf_render::GeoRenderOptions,
        foundation_snapshot: Option<&(ai_revit_core::EngineeringSnapshotId, ai_revit_core::GeoFoundationAnalysisSnapshot, ai_revit_core::EngineeringSnapshotStatus)>,
    ) -> String {
        let geo = projection.selected_snapshot.as_ref().map(|snapshot| format!(
            "{}:{}:{:?}:{}:{}",
            snapshot.selection_fingerprint, snapshot.content_fingerprint, projection.snapshot_currentness,
            snapshot.source_revision.operation_id, projection.snapshot_is_stored,
        )).unwrap_or_else(|| "no-geo-snapshot".into());
        let layout = projection.layout.as_ref().map(|layout| layout.input_fingerprint.as_str()).unwrap_or("no-layout");
        let foundation = foundation_snapshot.map(|(id, snapshot, status)| format!("{}:{}:{status:?}", id.get(), snapshot.content_fingerprint)).unwrap_or_else(|| "no-foundation-snapshot".into());
        format!("{}|{}|{}|{:?}|camera:{}|{}:{}:{}:{}:{}:{}|{:?}|{}|{:?}",
            projection.binding.document_uuid, projection.binding.runtime_id, projection.binding.open_generation,
            options.view, self.geo_foundation_ui.camera.revision, options.show_terrain, options.show_contacts, options.show_boreholes,
            options.show_unknown_zones, options.show_water, options.show_foundations,
            options.visible_ige_ids, options.section_max_spacing_m, options.selected_section,
        ) + &format!("|{geo}|{layout}|{foundation}|ground-map:{:?}|{:?}", self.geo_foundation_ui.ground_map_key, self.geo_foundation_ui.selection)
    }

    fn geo_foundation_render_changed(&mut self, cx: &mut Cx, force_fit: bool) {
        if !self.geo_foundation_ui.active { return; }
        let (projection, options, foundation_snapshot) = match self.geo_foundation_render_context() {
            Ok(context) => context,
            Err(message) => {
                self.geo_foundation_clear_scene(cx);
                self.ui.label(cx, ids!(gf_view_title)).set_text(cx, self.geo_foundation_ui.view.name());
                self.ui.label(cx, ids!(gf_render_status)).set_text(cx, &message);
                return;
            }
        };
        let Some(resolved) = projection.resolved.as_ref() else {
            self.geo_foundation_clear_scene(cx);
            self.ui.label(cx, ids!(gf_render_status)).set_text(cx, "MODEL_INCOMPLETE · no resolved source-bound model/scenario");
            return;
        };
        let foundation_ref = foundation_snapshot.as_ref().map(|(id, snapshot, status)| gf_render::FoundationSnapshotRef { id: *id, status: *status, snapshot });
        let selected = self.geo_foundation_ui.selection.as_ref();
        let input = gf_render::GeoFoundationRenderInput {
            state: &projection.state,
            resolved,
            snapshot_is_stored: projection.snapshot_is_stored,
            layout: projection.layout.as_ref(),
            foundation_snapshot: foundation_ref,
            ground_map: self.geo_foundation_ui.ground_map.as_ref(),
            options: &options,
            selection: selected,
        };
        let is_plan = matches!(options.view, GeoFoundationView::GroundPlan | GeoFoundationView::Section | GeoFoundationView::FoundationPlan | GeoFoundationView::Pressure2D | GeoFoundationView::Settlement2D);
        if is_plan {
            if let Some(canvas) = self.ui.drafting_canvas(cx, ids!(gf_canvas)).borrow() {
                let size = canvas.rect.size;
                if size.x > 1.0 && size.y > 1.0 {
                    self.geo_foundation_ui.canvas_measured = true;
                    if (self.geo_foundation_ui.camera.viewport_width - size.x).abs() > f64::EPSILON
                        || (self.geo_foundation_ui.camera.viewport_height - size.y).abs() > f64::EPSILON
                    {
                        let _ = self.geo_foundation_ui.camera.resize(size.x, size.y);
                        self.geo_foundation_ui.camera_change = Some(CanvasCameraChange::Resize);
                    }
                }
            }
        }
        let key = self.geo_foundation_render_key(&projection, &options, foundation_snapshot.as_ref());
        self.ui.label(cx, ids!(gf_view_title)).set_text(cx, options.view.name());
        if is_plan {
            self.ui.widget(cx, ids!(gf_canvas_host)).set_visible(cx, true);
            self.ui.widget(cx, ids!(gf_scene_host)).set_visible(cx, false);
            if self.geo_foundation_ui.screen_key.as_deref() == Some(key.as_str()) && !(force_fit || self.geo_foundation_ui.fit_pending) {
                return;
            }
            let bounds = match gf_render::scene(&input) {
                Ok(rendered) => {
                    if options.view == GeoFoundationView::Section {
                        self.geo_foundation_ui.selected_section.as_ref().and_then(|id| rendered.section_bounds.get(id)).copied().or(rendered.plan_bounds)
                    } else { rendered.plan_bounds }
                }
                Err(error) => {
                    self.geo_foundation_clear_scene(cx);
                    self.geo_foundation_show_status(cx, "RENDER_HOLD", &error);
                    self.ui.label(cx, ids!(gf_render_status)).set_text(cx, &format!("RENDER_HOLD · {error}"));
                    return;
                }
            };
            if (force_fit || self.geo_foundation_ui.fit_pending) && self.geo_foundation_ui.canvas_measured {
                if let Some(bounds) = bounds.and_then(|bounds| bounds.drafting_bounds()) {
                    if self.geo_foundation_ui.camera.fit_bounds(bounds, 28.0).is_ok() {
                        self.geo_foundation_ui.fit_pending = false;
                        self.geo_foundation_ui.camera_change = Some(CanvasCameraChange::Fit);
                    }
                }
            }
            let layer = if options.view == GeoFoundationView::Section {
                gf_render::section_layer(&input, &self.geo_foundation_ui.camera)
            } else {
                gf_render::plan_layer(&input, &self.geo_foundation_ui.camera)
            };
            match layer {
                Ok(layer) => {
                    let mut frame = CanvasFrame::default();
                    frame.snapshot_token = key.clone();
                    frame.source_generation = self.geo_foundation_ui.render_generation;
                    frame.model_generation = self.geo_foundation_ui.scene_generation;
                    frame.document_revision = projection.binding.document_revision;
                    frame.camera_revision = self.geo_foundation_ui.camera.revision;
                    frame.camera_change = self.geo_foundation_ui.camera_change.take();
                    let screen = Arc::new(layer);
                    frame.analysis = screen.clone();
                    if let Some(mut canvas) = self.ui.drafting_canvas(cx, ids!(gf_canvas)).borrow_mut() { canvas.set_frame(cx, frame); }
                    self.geo_foundation_ui.screen = screen;
                    self.geo_foundation_ui.screen_key = Some(key);
                    self.geo_foundation_ui.scene_key = None;
                    self.geo_foundation_ui.scene_picks.clear();
                    self.geo_foundation_ui.scene_sources.clear();
                    self.geo_foundation_ui.scene_source_fingerprint = None;
                    self.geo_foundation_ui.scene_foundation_snapshot_fingerprint = foundation_snapshot.as_ref().map(|(_, snapshot, _)| snapshot.content_fingerprint.clone());
                    let currentness = projection.snapshot_currentness.map_or("unknown", |value| match value { geo_core::SnapshotCurrentness::Current => "current closure", geo_core::SnapshotCurrentness::Stale => "stale closure" });
                    let persisted = if projection.snapshot_is_stored { "stored snapshot" } else { "read-only current-record projection" };
                    let layout_note = if projection.layout.is_some() && !self.geo_foundation_ui.layout_dirty { " · FoundationLayout linked by Core" } else { "" };
                    self.geo_foundation_ui.status = format!("SOURCE_BOUND_GEOMETRY · {currentness} · {persisted}{layout_note}");
                    self.ui.label(cx, ids!(gf_render_status)).set_text(cx, &self.geo_foundation_ui.status);
                }
                Err(error) => {
                    self.geo_foundation_ui.screen_key = None;
                    self.geo_foundation_clear_scene(cx);
                    self.ui.label(cx, ids!(gf_render_status)).set_text(cx, &format!("RENDER_HOLD · {error}"));
                }
            }
            return;
        }

        if self.geo_foundation_ui.scene_key.as_deref() == Some(key.as_str()) && !(force_fit || self.geo_foundation_ui.fit_pending) {
            return;
        }
        match gf_render::scene(&input) {
            Ok(rendered) => {
                let bounds = rendered.overall_bounds;
                let picks = rendered.picks.clone();
                let sources = rendered.object_sources.clone();
                let desired_visibility: BTreeMap<ObjectId, bool> = rendered.scene.objects().map(|object| (object.object_id, object.visible)).collect();
                let viewport_ref = self.ui.cad_viewport(cx, ids!(gf_viewport));
                let old_state = viewport_ref.borrow().map(|viewport| viewport.viewer().capture_state());
                let measured = viewport_ref.borrow().map(|viewport| viewport.rect().size);
                let mut viewer = Viewer::default();
                *viewer.scene_mut() = rendered.scene;
                if let Some(size) = measured.filter(|size| size.x > 1.0 && size.y > 1.0) {
                    let _ = viewer.set_viewport(CoreViewport::new(size.x, size.y));
                }
                if let Some(mut state) = old_state {
                    state.hidden_object_ids.clear();
                    state.isolated_object_ids.clear();
                    state.selected_object_ids.retain(|id| desired_visibility.get(id).copied() == Some(true));
                    state.highlighted_object_ids.retain(|id| desired_visibility.get(id).copied() == Some(true));
                    state.highlighted_object_id = state.highlighted_object_id.filter(|id| desired_visibility.get(id).copied() == Some(true));
                    if state.navigation_pivot.object_id.is_some_and(|id| desired_visibility.get(&id).copied() != Some(true)) { state.navigation_pivot.object_id = None; }
                    state.hover_hit = None;
                    let _ = viewer.restore_state(&state);
                }
                for (object_id, visible) in &desired_visibility { viewer.scene_mut().set_visible(*object_id, *visible); }
                let visible_selection = self.geo_foundation_ui.selection.as_ref().is_some_and(|selection| picks.values().any(|pick| pick == selection));
                if !visible_selection { self.geo_foundation_ui.selection = None; }
                viewport_ref.set_viewer(cx, viewer);
                if force_fit || self.geo_foundation_ui.fit_pending {
                    if let (Some(bounds), Some(size)) = (bounds, measured.filter(|size| size.x > 1.0 && size.y > 1.0)) {
                        let viewport_ref = self.ui.cad_viewport(cx, ids!(gf_viewport));
                        if let Some(mut widget) = viewport_ref.borrow_mut() {
                            let viewer = widget.viewer_mut();
                            let _ = viewer.set_viewport(CoreViewport::new(size.x, size.y));
                            if gf_render::fit_typed_bounds(viewer, bounds, 1.18).is_ok() { self.geo_foundation_ui.fit_pending = false; }
                        }
                    }
                }
                self.geo_foundation_ui.scene_picks = picks;
                self.geo_foundation_ui.scene_sources = sources;
                self.geo_foundation_ui.scene_source_fingerprint = Some(rendered.source_fingerprint.clone());
                self.geo_foundation_ui.scene_foundation_snapshot_fingerprint = foundation_snapshot.as_ref().map(|(_, snapshot, _)| snapshot.content_fingerprint.clone());
                self.geo_foundation_ui.scene_key = Some(key);
                self.geo_foundation_ui.screen_key = None;
                let mut statuses = rendered.status;
                statuses.push(format!("{} rendered scene object(s) · typed Geo picks only", desired_visibility.len()));
                self.geo_foundation_ui.status = statuses.join(" · ");
                self.ui.label(cx, ids!(gf_render_status)).set_text(cx, &self.geo_foundation_ui.status);
                self.geo_foundation_update_record_inspector(cx);
            }
            Err(error) => {
                self.geo_foundation_ui.scene_key = None;
                self.geo_foundation_clear_scene(cx);
                self.geo_foundation_show_status(cx, "RENDER_HOLD", &error);
                self.ui.label(cx, ids!(gf_render_status)).set_text(cx, &format!("RENDER_HOLD · {error}"));
            }
        }
    }

    fn geo_foundation_canvas_action(&mut self, cx: &mut Cx, action: CanvasAction) {
        let plan_view = matches!(self.geo_foundation_ui.view, GeoFoundationView::GroundPlan | GeoFoundationView::Section | GeoFoundationView::FoundationPlan | GeoFoundationView::Pressure2D | GeoFoundationView::Settlement2D);
        if !plan_view { return; }
        let mut changed = false;
        match action {
            CanvasAction::Resize { width, height, .. } => {
                if width > 1.0 && height > 1.0 && self.geo_foundation_ui.camera.resize(width, height).is_ok() {
                    self.geo_foundation_ui.canvas_measured = true;
                    self.geo_foundation_ui.camera_change = Some(CanvasCameraChange::Resize);
                    changed = true;
                }
            }
            CanvasAction::Input(CanvasInput::Scroll { screen, delta_y }) => {
                if delta_y.is_finite() && self.geo_foundation_ui.camera.zoom_at(screen, (-delta_y * 0.0015).clamp(-4.0, 4.0).exp()).is_ok() {
                    self.geo_foundation_ui.camera_change = Some(CanvasCameraChange::Zoom);
                    changed = true;
                }
            }
            CanvasAction::Input(CanvasInput::Pointer { phase, button, screen }) => match phase {
                PointerPhase::Down if button == PointerButton::Middle => self.geo_foundation_ui.pan_anchor = Some(screen),
                PointerPhase::Down if button == PointerButton::Left => {
                    self.geo_foundation_pick_plan(cx, screen);
                    changed = true;
                }
                PointerPhase::Move => {
                    if let Some(previous) = self.geo_foundation_ui.pan_anchor.replace(screen) {
                        if self.geo_foundation_ui.camera.pan_pixels(screen.x - previous.x, screen.y - previous.y).is_ok() {
                            self.geo_foundation_ui.camera_change = Some(CanvasCameraChange::Pan);
                            changed = true;
                        }
                    }
                }
                PointerPhase::Up | PointerPhase::Leave => self.geo_foundation_ui.pan_anchor = None,
                _ => {}
            },
            CanvasAction::Input(CanvasInput::Cancel) => self.geo_foundation_ui.pan_anchor = None,
            CanvasAction::Input(CanvasInput::Delete) => {
                self.geo_foundation_ui.selection = None;
                self.geo_foundation_update_record_inspector(cx);
                changed = true;
            }
            _ => {}
        }
        if changed { self.geo_foundation_render_changed(cx, false); }
    }

    fn geo_foundation_pick_plan(&mut self, cx: &mut Cx, screen: DraftPoint) {
        match self.geo_foundation_render_context() {
            Ok((projection, options, foundation_snapshot)) => {
                let Some(resolved) = projection.resolved.as_ref() else { return; };
                let foundation_ref = foundation_snapshot.as_ref().map(|(id, snapshot, status)| gf_render::FoundationSnapshotRef { id: *id, status: *status, snapshot });
                let input = gf_render::GeoFoundationRenderInput {
                    state: &projection.state, resolved, snapshot_is_stored: projection.snapshot_is_stored, layout: projection.layout.as_ref(),
                    foundation_snapshot: foundation_ref, ground_map: self.geo_foundation_ui.ground_map.as_ref(), options: &options,
                    selection: self.geo_foundation_ui.selection.as_ref(),
                };
                match gf_render::pick_plan(&input, &self.geo_foundation_ui.camera, screen, 8.0) {
                    Ok(selection) => self.geo_foundation_select_pick(cx, selection),
                    Err(error) => self.geo_foundation_show_status(cx, "PICK_HOLD", &error),
                }
            }
            Err(error) => {
                self.geo_foundation_ui.selection = None;
                self.geo_foundation_update_record_inspector(cx);
                self.geo_foundation_show_status(cx, "PICK_HOLD", &error);
            }
        }
    }

    fn geo_foundation_select_pick(&mut self, cx: &mut Cx, pick: Option<gf_render::GeoPick>) {
        self.geo_foundation_ui.selected_layer_volume_inspector = match pick.as_ref() {
            Some(layer_pick @ gf_render::GeoPick::LayerVolume { .. }) => self.geo_foundation_layer_volume_inspector(layer_pick),
            _ => None,
        };
        self.geo_foundation_ui.selected_ground_sample_inspector = match pick.as_ref() {
            Some(gf_render::GeoPick::FoundationGroundSample { foundation_id, index }) => {
                self.geo_foundation_ground_sample_inspector(foundation_id, *index).and_then(|text| {
                    self.geo_foundation_ui.ground_map_key.clone().map(|key| (key, text))
                })
            }
            _ => None,
        };
        self.geo_foundation_ui.selection = pick.clone();
        match pick {
            Some(gf_render::GeoPick::Borehole { model_id, borehole_id }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_borehole = Some(borehole_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Boreholes;
            }
            Some(gf_render::GeoPick::BoreholeInterval { model_id, borehole_id, interval_id }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_borehole = Some(borehole_id);
                self.geo_foundation_ui.selected_interval = Some(interval_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Boreholes;
            }
            Some(gf_render::GeoPick::TerrainTriangle { model_id, triangle_id }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_triangle = Some(triangle_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Model;
            }
            Some(gf_render::GeoPick::LayerVolume { model_id, triangle_id, horizon_id, ige_id }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_triangle = Some(triangle_id);
                self.geo_foundation_ui.selected_contact = Some(horizon_id);
                self.geo_foundation_ui.selected_ige = Some(ige_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Contacts;
            }
            Some(gf_render::GeoPick::Contact { model_id, horizon_id, ige_id }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_contact = Some(horizon_id);
                self.geo_foundation_ui.selected_ige = Some(ige_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Contacts;
            }
            Some(gf_render::GeoPick::ContactUnknown { model_id, horizon_id, borehole_id, .. }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_contact = Some(horizon_id);
                self.geo_foundation_ui.selected_contact_borehole = Some(borehole_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Contacts;
            }
            Some(gf_render::GeoPick::Section { model_id, section_id }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_section = Some(section_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Sections;
            }
            Some(gf_render::GeoPick::Ige { model_id, ige_id }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_ige = Some(ige_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Iges;
            }
            Some(gf_render::GeoPick::Soil { model_id, soil_id }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_soil = Some(soil_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Soils;
            }
            Some(gf_render::GeoPick::UnknownZone { model_id, index }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.unknown_zone_index = Some(index);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::UnknownZones;
            }
            Some(gf_render::GeoPick::WaterObservation { model_id, scenario_id, observation_id }) => {
                self.geo_foundation_ui.selected_model = Some(model_id);
                self.geo_foundation_ui.selected_scenario = Some(scenario_id);
                self.geo_foundation_ui.selected_water_observation = Some(observation_id);
                self.geo_foundation_ui.water_editing_observation = true;
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Water;
            }
            Some(gf_render::GeoPick::FoundationPlacement { placement_id, .. }) => {
                self.geo_foundation_ui.selected_placement = FoundationId::new(placement_id).ok();
                self.geo_foundation_ui.selected_foundation = self.geo_foundation_ui.selected_placement.as_ref().map(ToString::to_string);
                if let Some(selected_id) = self.geo_foundation_ui.selected_placement.as_ref().map(ToString::to_string) {
                    self.geo_foundation_clear_ground_map_for_placement(cx, &selected_id);
                }
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Model;
            }
            Some(gf_render::GeoPick::FoundationTier { placement_id, tier_id, .. }) => {
                self.geo_foundation_ui.selected_placement = FoundationId::new(placement_id).ok();
                self.geo_foundation_ui.selected_foundation = self.geo_foundation_ui.selected_placement.as_ref().map(ToString::to_string);
                if let Some(selected_id) = self.geo_foundation_ui.selected_placement.as_ref().map(ToString::to_string) {
                    self.geo_foundation_clear_ground_map_for_placement(cx, &selected_id);
                }
                self.geo_foundation_ui.selected_tier_id = Some(tier_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Model;
            }
            Some(gf_render::GeoPick::FoundationCell { foundation_id, .. }) => {
                self.geo_foundation_ui.selected_foundation = Some(foundation_id.to_string());
                self.geo_foundation_ui.selected_foundation_snapshot = self.geo_foundation_ui.projection.as_ref().and_then(|projection| {
                    projection.foundation_snapshots.iter().find(|(_, snapshot, status)| snapshot.input.effective_input.foundation_id == foundation_id && *status == ai_revit_core::EngineeringSnapshotStatus::Current)
                        .map(|(id, _, _)| *id)
                });
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Model;
            }
            Some(gf_render::GeoPick::FoundationGroundSample { foundation_id, .. }) => {
                self.geo_foundation_ui.selected_foundation = Some(foundation_id);
                self.geo_foundation_ui.active_tab = GeoFoundationTab::Model;
            }
            None => {}
        }
        self.geo_foundation_set_tab_host(cx);
        self.geo_foundation_sync_selectors(cx);
        if !self.geo_foundation_ui.draft_dirty && !self.geo_foundation_ui.layout_dirty {
            self.geo_foundation_load_selected_editor(cx);
            self.geo_foundation_update_record_inspector(cx);
        }
    }

    fn geo_foundation_clear_ground_map_for_placement(&mut self, cx: &mut Cx, placement_id: &str) {
        if self.geo_foundation_ui.ground_map.as_ref()
            .is_some_and(|map| map.foundation_id.as_str() != placement_id)
        {
            self.geo_foundation_ui.ground_map = None;
            self.geo_foundation_ui.ground_map_key = None;
            self.geo_foundation_ui.ground_map_status = format!(
                "Placement selection changed to {placement_id} · request a new source-bound midpoint map explicitly."
            );
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
        }
    }


    fn geo_foundation_refresh_projection(&mut self, cx: &mut Cx) {
        let Some(document) = self.document.as_ref() else {
            let ui = &mut self.geo_foundation_ui;
            ui.projection = None;
            ui.binding = None;
            ui.binding_stale = ui.draft_dirty || ui.layout_dirty;
            ui.analysis_controls_stale = ui.analysis_controls_dirty || ui.analysis_controls_binding.is_some();
            ui.model_ids.clear();
            ui.source_ids.clear();
            ui.soil_ids.clear();
            ui.ige_ids.clear();
            ui.borehole_ids.clear();
            ui.scenario_ids.clear();
            ui.section_ids.clear();
            ui.snapshot_ids.clear();
            ui.foundation_snapshot_ids.clear();
            ui.control_template_ids.clear();
            ui.configured_load_source_ids.clear();
            ui.selected_control_template = None;
            ui.selected_configured_load_source = None;
            ui.ground_map = None;
            ui.ground_map_key = None;
            ui.ground_map_status = "MODEL_INCOMPLETE · no current Document for a ground map".into();
            ui.triangle_ids.clear();
            ui.contact_ids.clear();
            ui.water_observation_ids.clear();
            ui.interval_ids.clear();
            ui.model_borehole_ids.clear();
            ui.selected_model = None;
            ui.selected_scenario = None;
            ui.selected_snapshot = None;
            ui.selected_foundation_snapshot = None;
            ui.clear_current_results();
            ui.status = "MODEL_INCOMPLETE · no current Document".into();
            ui.report_text = "No current Document is open. No saved Geo/Foundation result is displayed.".into();
            self.ui.label(cx, ids!(gf_binding_label)).set_text(cx, "No Document open");
            self.ui.label(cx, ids!(gf_status)).set_text(cx, &self.geo_foundation_ui.status);
            self.ui.label(cx, ids!(gf_snapshot_status)).set_text(cx, "No Document · no selected snapshot");
            self.ui.text_input(cx, ids!(gf_inspector_text)).set_text(cx, "No Geo record selected");
            self.ui.text_input(cx, ids!(gf_report_text)).set_text(cx, &self.geo_foundation_ui.report_text);
            self.ui.text_input(cx, ids!(gf_query_text)).set_text(cx, &self.geo_foundation_ui.query_text);
            self.ui.text_input(cx, ids!(gf_layout_status)).set_text(cx, "No current Document layout.");
            self.ui.drop_down(cx, ids!(gf_snapshot_select)).set_labels(cx, vec!["Current records · no Document".to_owned()]);
            self.ui.drop_down(cx, ids!(gf_model_select)).set_labels(cx, vec!["No current ground models".to_owned()]);
            self.ui.drop_down(cx, ids!(gf_scenario_select)).set_labels(cx, vec!["No current scenarios".to_owned()]);
            self.ui.drop_down(cx, ids!(gf_visible_ige)).set_labels(cx, vec!["All IGEs · no current records".to_owned()]);
            self.ui.drop_down(cx, ids!(gf_foundation_snapshot_select)).set_labels(cx, vec!["No current foundation snapshots".to_owned()]);
            self.ui.drop_down(cx, ids!(gf_control_template_select)).set_labels(cx, vec!["No current numerical control templates".to_owned()]);
            self.ui.drop_down(cx, ids!(gf_configured_load_select)).set_labels(cx, vec!["No current configured load sources".to_owned()]);
            self.ui.text_input(cx, ids!(gf_ground_map_status)).set_text(cx, &self.geo_foundation_ui.ground_map_status);
            if !self.geo_foundation_ui.draft_dirty && !self.geo_foundation_ui.layout_dirty {
                self.geo_foundation_clear_editor(cx);
            }
            self.geo_foundation_apply_visibility(cx);
            self.geo_foundation_clear_scene(cx);
            return;
        };

        let current_geo = document.geo_state();
        if !self.geo_foundation_ui.draft_dirty {
            if let Some(snapshot) = self.geo_foundation_ui.selected_snapshot.as_ref()
                .and_then(|id| current_geo.snapshots.get(id))
            {
                self.geo_foundation_ui.selected_model = Some(snapshot.selection.model_id.clone());
                self.geo_foundation_ui.selected_scenario = snapshot.selection.scenario_id.clone();
            } else {
            if self.geo_foundation_ui.selected_model.as_ref().is_none_or(|id| !current_geo.records.models.contains_key(id)) {
                self.geo_foundation_ui.selected_model = current_geo.records.models.keys().next().cloned();
            }
            if self.geo_foundation_ui.selected_scenario.as_ref().is_some_and(|id| {
                current_geo.records.scenarios.get(id).is_none_or(|scenario| Some(&scenario.model_id) != self.geo_foundation_ui.selected_model.as_ref())
            }) {
                self.geo_foundation_ui.selected_scenario = None;
            }
            }
        }

        let selection = self.geo_foundation_ui.selected_model.clone()
            .filter(|model_id| current_geo.records.models.contains_key(model_id)).map(|model_id| {
            GeoSnapshotSelection {
                model_id,
                scenario_id: self.geo_foundation_ui.selected_scenario.clone(),
            }
        });
        let projection = gf_api::projection(
            document,
            self.api_document_generation,
            selection.as_ref(),
            self.geo_foundation_ui.selected_snapshot.as_ref(),
        );

        match projection {
            Ok(projection) => {
                let next_query_identity = geo_foundation_query_identity(&projection);
                let ui = &mut self.geo_foundation_ui;
                let query_identity_changed = ui.query_result_binding.as_ref()
                    .is_some_and(|previous| previous != &next_query_identity);
                let draft_binding_changed = ui.draft_dirty
                    && ui.binding.as_ref().is_some_and(|binding| binding != &projection.binding);
                let layout_binding_changed = ui.layout_dirty
                    && ui.layout_binding.as_ref().is_some_and(|binding| binding != &projection.binding);
                let analysis_binding_changed = ui.analysis_controls_binding.as_ref()
                    .is_some_and(|binding| binding != &projection.binding);
                if !ui.draft_dirty && !ui.layout_dirty {
                    ui.binding = Some(projection.binding.clone());
                    ui.layout_binding = Some(projection.binding.clone());
                }
                ui.binding_stale = draft_binding_changed || layout_binding_changed;
                ui.analysis_controls_stale = analysis_binding_changed;
                ui.status = if ui.binding_stale {
                    "STALE · Document/GEO records changed; reload before applying this draft".into()
                } else {
                    projection.status.clone()
                };
                ui.report_text = projection.report.clone();
                ui.projection = Some(projection);
                if query_identity_changed {
                    ui.clear_current_results();
                }
                self.geo_foundation_sync_selectors(cx);
                self.geo_foundation_update_header(cx);
                if !self.geo_foundation_ui.draft_dirty && !self.geo_foundation_ui.layout_dirty {
                    self.geo_foundation_load_selected_editor(cx);
                }
                self.geo_foundation_update_record_inspector(cx);
            }
            Err(error) => {
                let ui = &mut self.geo_foundation_ui;
                ui.projection = None;
                ui.binding_stale = ui.draft_dirty || ui.layout_dirty;
                ui.analysis_controls_stale = ui.analysis_controls_dirty || ui.analysis_controls_binding.is_some();
                ui.status = if ui.binding_stale {
                    "STALE · current Document projection unavailable; reload before applying".into()
                } else {
                    format!("MODEL_INCOMPLETE · {error}")
                };
                ui.report_text = format!("No current projection: {error}");
                ui.clear_current_results();
                self.geo_foundation_update_header(cx);
                self.geo_foundation_clear_scene(cx);
                self.ui.text_input(cx, ids!(gf_query_text)).set_text(cx, &self.geo_foundation_ui.query_text);
                self.ui.text_input(cx, ids!(gf_inspector_text)).set_text(cx, &self.geo_foundation_ui.report_text);
            }
        }
    }

    fn geo_foundation_sync_selectors(&mut self, cx: &mut Cx) {
        let Some(projection) = self.geo_foundation_ui.projection.as_ref() else { return; };
        let state = &projection.state;

        self.geo_foundation_ui.snapshot_ids = state.snapshots.keys().cloned().collect();
        self.geo_foundation_ui.model_ids = state.records.models.keys().cloned().collect();
        self.geo_foundation_ui.source_ids = state.records.sources.keys().cloned().collect();
        self.geo_foundation_ui.soil_ids = state.records.soils.keys().cloned().collect();
        self.geo_foundation_ui.ige_ids = state.records.iges.keys().cloned().collect();
        self.geo_foundation_ui.borehole_ids = state.records.boreholes.keys().cloned().collect();
        self.geo_foundation_ui.scenario_ids = state.records.scenarios.keys().cloned().collect();
        self.geo_foundation_ui.water_observation_ids = state.records.water_observations.keys().cloned().collect();
        self.geo_foundation_ui.section_ids = state.records.sections.keys().cloned().collect();
        let draft_model = self.geo_foundation_ui.draft.as_ref().and_then(|draft| match draft {
            GeoFoundationDraft::Model(model) => Some(model.clone()),
            _ => None,
        });
        let current_model = draft_model.or_else(|| self.geo_foundation_ui.selected_model.as_ref()
            .and_then(|id| state.records.models.get(id)).cloned());
        self.geo_foundation_ui.triangle_ids = current_model.as_ref()
            .map(|model| model.triangles.iter().map(|triangle| triangle.id.clone()).collect())
            .unwrap_or_default();
        self.geo_foundation_ui.contact_ids = current_model.as_ref()
            .map(|model| model.contacts.iter().map(|contact| contact.id.clone()).collect())
            .unwrap_or_default();
        self.geo_foundation_ui.model_borehole_ids = current_model.as_ref()
            .map(|model| model.borehole_ids.clone()).unwrap_or_default();
        self.geo_foundation_ui.interval_ids = self.geo_foundation_ui.selected_borehole.as_ref()
            .and_then(|id| state.records.boreholes.get(id))
            .map(|borehole| borehole.intervals.iter().map(|interval| interval.id.clone()).collect())
            .unwrap_or_default();
        self.geo_foundation_ui.selected_source = self.geo_foundation_ui.selected_source.as_ref()
            .filter(|id| state.records.sources.contains_key(*id)).cloned()
            .or_else(|| self.geo_foundation_ui.source_ids.first().cloned());
        self.geo_foundation_ui.selected_soil = self.geo_foundation_ui.selected_soil.as_ref()
            .filter(|id| state.records.soils.contains_key(*id)).cloned()
            .or_else(|| self.geo_foundation_ui.soil_ids.first().cloned());
        self.geo_foundation_ui.selected_ige = self.geo_foundation_ui.selected_ige.as_ref()
            .filter(|id| state.records.iges.contains_key(*id)).cloned()
            .or_else(|| self.geo_foundation_ui.ige_ids.first().cloned());
        self.geo_foundation_ui.visible_ige_filter = self.geo_foundation_ui.visible_ige_filter.as_ref()
            .filter(|id| state.records.iges.contains_key(*id)).cloned();
        self.geo_foundation_ui.selected_borehole = self.geo_foundation_ui.selected_borehole.as_ref()
            .filter(|id| state.records.boreholes.contains_key(*id)).cloned()
            .or_else(|| self.geo_foundation_ui.borehole_ids.first().cloned());
        self.geo_foundation_ui.selected_triangle = self.geo_foundation_ui.selected_triangle.as_ref()
            .filter(|id| self.geo_foundation_ui.triangle_ids.contains(*id)).cloned()
            .or_else(|| self.geo_foundation_ui.triangle_ids.first().cloned());
        self.geo_foundation_ui.selected_contact = self.geo_foundation_ui.selected_contact.as_ref()
            .filter(|id| self.geo_foundation_ui.contact_ids.contains(*id)).cloned()
            .or_else(|| self.geo_foundation_ui.contact_ids.first().cloned());
        self.geo_foundation_ui.selected_section = self.geo_foundation_ui.selected_section.as_ref()
            .filter(|id| state.records.sections.contains_key(*id)).cloned()
            .or_else(|| self.geo_foundation_ui.section_ids.first().cloned());
        self.geo_foundation_ui.selected_water_observation = self.geo_foundation_ui.selected_water_observation.as_ref()
            .filter(|id| state.records.water_observations.contains_key(*id)).cloned()
            .or_else(|| self.geo_foundation_ui.water_observation_ids.first().cloned());
        self.geo_foundation_ui.selected_interval = self.geo_foundation_ui.selected_interval.as_ref()
            .filter(|id| self.geo_foundation_ui.interval_ids.contains(*id)).cloned()
            .or_else(|| self.geo_foundation_ui.interval_ids.first().cloned());
        if self.geo_foundation_ui.unknown_zone_index.is_some_and(|index| {
            current_model.as_ref().is_none_or(|model| index >= model.unknown_zones.len())
        }) {
            self.geo_foundation_ui.unknown_zone_index = None;
        }
        if self.geo_foundation_ui.unknown_zone_index.is_none() {
            self.geo_foundation_ui.unknown_zone_index = current_model.as_ref()
                .and_then(|model| (!model.unknown_zones.is_empty()).then_some(0));
        }

        let model_labels: Vec<String> = state.records.models.iter()
            .map(|(id, model)| format!("{} · {}", id, model.name)).collect();
        let scenario_labels: Vec<String> = state.records.scenarios.iter()
            .map(|(id, scenario)| format!("{} · {}", id, scenario.name)).collect();
        let mut snapshot_labels: Vec<String> = vec!["Current records · read-only view".to_owned()];
        snapshot_labels.extend(state.snapshots.iter().map(|(id, snapshot)| {
            let scenario = snapshot.selection.scenario_id.as_ref().map_or("no scenario", |id| id.as_str());
            format!("{} · model {} · {} · source op {}", id, snapshot.selection.model_id, scenario, snapshot.source_revision.operation_id)
        }));
        let source_labels: Vec<String> = state.records.sources.iter()
            .map(|(id, record)| format!("{} · {}", id, record.title)).collect();
        let soil_labels: Vec<String> = state.records.soils.iter()
            .map(|(id, record)| format!("{} · {}", id, record.name)).collect();
        let ige_labels: Vec<String> = state.records.iges.iter()
            .map(|(id, record)| format!("{} · {}", id, record.name)).collect();
        let borehole_labels: Vec<String> = state.records.boreholes.iter().map(|(id, borehole)| {
            format!("{} · ({:.3}, {:.3}, {:.3}) m", id, borehole.collar_m[0], borehole.collar_m[1], borehole.collar_m[2])
        }).collect();
        let triangle_labels: Vec<String> = current_model.as_ref()
            .map(|model| model.triangles.iter().map(|triangle| {
                format!("{} · {} / {} / {}", triangle.id, triangle.vertices[0], triangle.vertices[1], triangle.vertices[2])
            }).collect()).unwrap_or_default();
        let contact_labels: Vec<String> = current_model.as_ref()
            .map(|model| model.contacts.iter().map(|contact| format!("{} · IGE {}", contact.id, contact.ige_id)).collect())
            .unwrap_or_default();
        let unknown_labels: Vec<String> = current_model.as_ref()
            .map(|model| model.unknown_zones.iter().enumerate().map(|(index, zone)| {
                format!("Zone {} · {}", index + 1, zone.reason)
            }).collect()).unwrap_or_default();
        let water_labels: Vec<String> = state.records.water_observations.iter()
            .map(|(id, observation)| format!("{} · XY ({:.3}, {:.3}) m", id, observation.xy_m[0], observation.xy_m[1]))
            .collect();
        let section_labels: Vec<String> = state.records.sections.iter()
            .map(|(id, section)| format!("{} · model {} · {} points", id, section.model_id, section.polyline_xy_m.len()))
            .collect();
        let interval_labels: Vec<String> = self.geo_foundation_ui.selected_borehole.as_ref()
            .and_then(|id| state.records.boreholes.get(id))
            .map(|borehole| borehole.intervals.iter().map(|interval| {
                format!("{} · {:.3}–{:.3} m · IGE {}", interval.id, interval.top_depth_m, interval.bottom_depth_m, interval.ige_id)
            }).collect()).unwrap_or_default();
        let model_borehole_labels: Vec<String> = self.geo_foundation_ui.model_borehole_ids.iter()
            .map(|id| id.to_string()).collect();
        let snapshot_index = self.geo_foundation_ui.selected_snapshot.as_ref()
            .and_then(|id| self.geo_foundation_ui.snapshot_ids.iter().position(|value| value == id))
            .map_or(0, |index| index + 1);
        let model_index = selector_index(&state.records.models.keys().cloned().collect::<Vec<_>>(), self.geo_foundation_ui.selected_model.as_ref());
        let scenario_index = selector_index(&self.geo_foundation_ui.scenario_ids, self.geo_foundation_ui.selected_scenario.as_ref());

        self.ui.drop_down(cx, ids!(gf_snapshot_select)).set_labels(cx, selector_labels_or_empty(snapshot_labels, "No saved snapshots"));
        self.ui.drop_down(cx, ids!(gf_snapshot_select)).set_selected_item(cx, snapshot_index);
        self.ui.drop_down(cx, ids!(gf_model_select)).set_labels(cx, selector_labels_or_empty(model_labels, "No stored ground models"));
        self.ui.drop_down(cx, ids!(gf_model_select)).set_selected_item(cx, model_index);
        let mut scenario_options: Vec<String> = vec!["No scenario selected · water remains unspecified".to_owned()];
        scenario_options.extend(scenario_labels);
        self.ui.drop_down(cx, ids!(gf_scenario_select)).set_labels(cx, scenario_options);
        self.ui.drop_down(cx, ids!(gf_scenario_select)).set_selected_item(cx, if self.geo_foundation_ui.selected_scenario.is_some() { scenario_index + 1 } else { 0 });
        self.ui.drop_down(cx, ids!(gf_source_select)).set_labels(cx, selector_labels_or_empty(source_labels, "No stored sources"));
        self.ui.drop_down(cx, ids!(gf_source_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.source_ids, self.geo_foundation_ui.selected_source.as_ref()));
        let mut bedding_source_labels: Vec<String> = vec!["Choose a source in the selected GEO snapshot closure".to_owned()];
        self.geo_foundation_ui.bedding_source_option_ids = self.geo_foundation_ui.projection.as_ref()
            .and_then(|projection| projection.selected_snapshot.as_ref())
            .map(|snapshot| snapshot.records.sources.keys()
                .filter(|id| state.records.sources.contains_key(*id)).cloned().collect())
            .unwrap_or_default();
        // These IDs come from the exact selected closure; the placeholder remains the default.
        bedding_source_labels.extend(self.geo_foundation_ui.bedding_source_option_ids.iter().filter_map(|id| {
            state.records.sources.get(id).map(|source| format!("{} · {}", id, source.title))
        }));
        self.ui.drop_down(cx, ids!(gf_bedding_source_select)).set_labels(cx, bedding_source_labels);
        let bedding_source_index = self.geo_foundation_ui.selected_bedding_source.as_ref()
            .map_or(0, |id| selector_index(&self.geo_foundation_ui.bedding_source_option_ids, Some(id)) + 1);
        self.ui.drop_down(cx, ids!(gf_bedding_source_select)).set_selected_item(cx, bedding_source_index);
        self.ui.drop_down(cx, ids!(gf_soil_select)).set_labels(cx, selector_labels_or_empty(soil_labels, "No stored GeoSoilRecords"));
        self.ui.drop_down(cx, ids!(gf_soil_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.soil_ids, self.geo_foundation_ui.selected_soil.as_ref()));
        self.ui.drop_down(cx, ids!(gf_ige_select)).set_labels(cx, selector_labels_or_empty(ige_labels.clone(), "No stored IGEs"));
        self.ui.drop_down(cx, ids!(gf_ige_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.ige_ids, self.geo_foundation_ui.selected_ige.as_ref()));
        let mut visible_ige_labels: Vec<String> = vec!["All IGEs".to_owned()];
        visible_ige_labels.extend(state.records.iges.iter().map(|(id, ige)| format!("{} · {}", id, ige.name)));
        self.ui.drop_down(cx, ids!(gf_visible_ige)).set_labels(cx, visible_ige_labels);
        self.ui.drop_down(cx, ids!(gf_visible_ige)).set_selected_item(cx, self.geo_foundation_ui.visible_ige_filter.as_ref()
            .and_then(|id| self.geo_foundation_ui.ige_ids.iter().position(|value| value == id)).map_or(0, |index| index + 1));
        self.ui.drop_down(cx, ids!(gf_ige_soil)).set_labels(cx, selector_labels_or_empty(self.geo_foundation_ui.soil_ids.iter().map(|id| id.to_string()).collect::<Vec<String>>(), "No stored soils"));
        self.ui.drop_down(cx, ids!(gf_borehole_select)).set_labels(cx, selector_labels_or_empty(borehole_labels.clone(), "No stored boreholes"));
        self.ui.drop_down(cx, ids!(gf_borehole_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.borehole_ids, self.geo_foundation_ui.selected_borehole.as_ref()));
        self.ui.drop_down(cx, ids!(gf_interval_select)).set_labels(cx, selector_labels_or_empty(interval_labels, "No intervals"));
        self.ui.drop_down(cx, ids!(gf_interval_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.interval_ids, self.geo_foundation_ui.selected_interval.as_ref()));
        self.ui.drop_down(cx, ids!(gf_interval_ige)).set_labels(cx, selector_labels_or_empty(self.geo_foundation_ui.ige_ids.iter().map(|id| id.to_string()).collect::<Vec<String>>(), "No stored IGEs"));
        self.ui.drop_down(cx, ids!(gf_model_borehole_select)).set_labels(cx, selector_labels_or_empty(borehole_labels.clone(), "No stored boreholes"));
        self.ui.drop_down(cx, ids!(gf_model_borehole_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.borehole_ids, self.geo_foundation_ui.selected_borehole.as_ref()));
        self.ui.drop_down(cx, ids!(gf_triangle_select)).set_labels(cx, selector_labels_or_empty(triangle_labels, "No stored triangles"));
        self.ui.drop_down(cx, ids!(gf_triangle_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.triangle_ids, self.geo_foundation_ui.selected_triangle.as_ref()));
        self.ui.drop_down(cx, ids!(gf_triangle_vertex_a)).set_labels(cx, selector_labels_or_empty(model_borehole_labels.clone(), "No model boreholes"));
        self.ui.drop_down(cx, ids!(gf_triangle_vertex_b)).set_labels(cx, selector_labels_or_empty(model_borehole_labels.clone(), "No model boreholes"));
        self.ui.drop_down(cx, ids!(gf_triangle_vertex_c)).set_labels(cx, selector_labels_or_empty(model_borehole_labels.clone(), "No model boreholes"));
        self.ui.drop_down(cx, ids!(gf_contact_select)).set_labels(cx, selector_labels_or_empty(contact_labels, "No stored contacts"));
        self.ui.drop_down(cx, ids!(gf_contact_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.contact_ids, self.geo_foundation_ui.selected_contact.as_ref()));
        self.ui.drop_down(cx, ids!(gf_contact_ige)).set_labels(cx, selector_labels_or_empty(self.geo_foundation_ui.ige_ids.iter().map(|id| id.to_string()).collect::<Vec<String>>(), "No stored IGEs"));
        self.ui.drop_down(cx, ids!(gf_contact_borehole)).set_labels(cx, selector_labels_or_empty(self.geo_foundation_ui.model_borehole_ids.iter().map(|id| id.to_string()).collect::<Vec<String>>(), "No model boreholes"));
        self.ui.drop_down(cx, ids!(gf_unknown_select)).set_labels(cx, selector_labels_or_empty(unknown_labels, "No unknown zones"));
        self.ui.drop_down(cx, ids!(gf_water_observation_select)).set_labels(cx, selector_labels_or_empty(water_labels, "No stored water observations"));
        self.ui.drop_down(cx, ids!(gf_water_observation_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.water_observation_ids, self.geo_foundation_ui.selected_water_observation.as_ref()));
        self.ui.drop_down(cx, ids!(gf_scenario_model)).set_labels(cx, selector_labels_or_empty(state.records.models.iter().map(|(id, model)| format!("{} · {}", id, model.name)).collect::<Vec<String>>(), "Select a stored model"));
        self.ui.drop_down(cx, ids!(gf_scenario_model)).set_selected_item(cx, model_index);
        self.ui.drop_down(cx, ids!(gf_section_select)).set_labels(cx, selector_labels_or_empty(section_labels, "No stored interpreted sections"));
        self.ui.drop_down(cx, ids!(gf_section_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.section_ids, self.geo_foundation_ui.selected_section.as_ref()));
        self.ui.drop_down(cx, ids!(gf_section_model)).set_labels(cx, selector_labels_or_empty(state.records.models.keys().map(ToString::to_string).collect::<Vec<String>>(), "No stored ground models"));
        self.geo_foundation_sync_foundation_selectors(cx);
    }

    fn geo_foundation_sync_foundation_selectors(&mut self, cx: &mut Cx) {
        let (layout_state, state, binding, control_templates, configured_load_sources) = match self.geo_foundation_ui.projection.as_ref() {
            Some(projection) => (projection.layout.clone(), projection.state.clone(), Some(projection.binding.clone()), projection.control_templates.clone(), projection.configured_load_sources.clone()),
            None => (None, GeoState::default(), self.geo_foundation_ui.binding.clone(), Vec::new(), Vec::new()),
        };
        let foundation_snapshot_items = self.geo_foundation_ui.projection.as_ref()
            .map(|projection| projection.foundation_snapshots.iter().map(|(id, snapshot, status)| {
                ((*id).clone(), snapshot, *status)
            }).collect::<Vec<_>>()).unwrap_or_default();
        self.geo_foundation_ui.foundation_snapshot_ids = foundation_snapshot_items.iter().map(|(id, _, _)| (*id).clone()).collect();
        self.geo_foundation_ui.selected_foundation_snapshot = self.geo_foundation_ui.selected_foundation_snapshot
            .filter(|id| self.geo_foundation_ui.foundation_snapshot_ids.contains(id));
        if !self.geo_foundation_ui.layout_dirty {
            self.geo_foundation_ui.layout_binding = binding;
            self.geo_foundation_ui.layout_draft = layout_state.as_ref().map(|layout| layout.input.layout.clone());
            if self.geo_foundation_ui.layout_draft.is_none() {
                self.geo_foundation_ui.layout_link_model = None;
                self.geo_foundation_ui.layout_link_scenario = None;
            }
        }

        let layout = self.geo_foundation_ui.layout_draft.clone();
        self.geo_foundation_ui.placement_ids = layout.as_ref().map(|layout| {
            layout.elements.iter().map(|placement| foundation_placement_id(placement).clone()).collect()
        }).unwrap_or_default();
        self.geo_foundation_ui.selected_placement = self.geo_foundation_ui.selected_placement.as_ref()
            .filter(|id| self.geo_foundation_ui.placement_ids.contains(id)).cloned()
            .or_else(|| self.geo_foundation_ui.placement_ids.first().cloned());
        self.geo_foundation_ui.selected_foundation = self.geo_foundation_ui.selected_placement.as_ref().map(ToString::to_string);
        let selected_foundation_id = self.geo_foundation_ui.selected_placement.clone();
        self.geo_foundation_ui.control_template_ids = control_templates.iter()
            .filter_map(|(id, _)| (Some(id) == selected_foundation_id.as_ref()).then_some(id.clone()))
            .collect();
        if !self.geo_foundation_ui.analysis_controls_dirty {
            self.geo_foundation_ui.selected_control_template = self.geo_foundation_ui.selected_control_template.as_ref()
                .filter(|id| self.geo_foundation_ui.control_template_ids.contains(id)).cloned();
            self.geo_foundation_ui.selected_configured_load_source = self.geo_foundation_ui.selected_configured_load_source
                .filter(|id| configured_load_sources.iter().any(|source| source.snapshot_id == *id && selected_foundation_id.as_ref().is_some_and(|foundation_id| source.foundation_ids.contains(foundation_id))));
        }
        self.geo_foundation_ui.configured_load_source_ids = configured_load_sources.iter()
            .filter_map(|source| selected_foundation_id.as_ref().filter(|foundation_id| source.foundation_ids.contains(*foundation_id)).map(|_| source.snapshot_id))
            .collect();
        let mut control_template_labels: Vec<String> = vec!["Choose existing numerical control template".to_owned()];
        control_template_labels.extend(control_templates.iter().filter(|(id, _)| Some(id) == selected_foundation_id.as_ref())
            .map(|(id, input)| format!("{} · declared synthetic numerical controls · mesh {} × {}", id, input.mesh.nx, input.mesh.ny)));
        self.ui.drop_down(cx, ids!(gf_control_template_select)).set_labels(cx, control_template_labels);
        self.ui.drop_down(cx, ids!(gf_control_template_select)).set_selected_item(cx, self.geo_foundation_ui.selected_control_template.as_ref()
            .map_or(0, |id| selector_index(&self.geo_foundation_ui.control_template_ids, Some(id)) + 1));
        let mut configured_load_labels: Vec<String> = vec!["Choose saved configured load source".to_owned()];
        configured_load_labels.extend(configured_load_sources.iter()
            .filter(|source| selected_foundation_id.as_ref().is_some_and(|id| source.foundation_ids.contains(id)))
            .map(|source| format!("{} · {:?} · {}", source.snapshot_id.get(), source.status, source.content_fingerprint)));
        self.ui.drop_down(cx, ids!(gf_configured_load_select)).set_labels(cx, configured_load_labels);
        self.ui.drop_down(cx, ids!(gf_configured_load_select)).set_selected_item(cx, self.geo_foundation_ui.selected_configured_load_source.as_ref()
            .map_or(0, |id| selector_index(&self.geo_foundation_ui.configured_load_source_ids, Some(id)) + 1));
        self.geo_foundation_ui.tier_ids = layout.as_ref()
            .and_then(|layout| self.geo_foundation_ui.selected_placement.as_ref()
                .and_then(|id| layout.elements.iter().find(|placement| foundation_placement_id(placement) == id)))
            .and_then(|placement| match placement {
                FoundationPlacement::Stepped { tiers, .. } => Some(tiers.iter().map(|tier| tier.id.clone()).collect()),
                _ => None,
            }).unwrap_or_default();
        self.geo_foundation_ui.selected_tier_id = self.geo_foundation_ui.selected_tier_id.as_ref()
            .filter(|id| self.geo_foundation_ui.tier_ids.contains(id)).cloned()
            .or_else(|| self.geo_foundation_ui.tier_ids.first().cloned());
        if !self.geo_foundation_ui.layout_dirty {
            let link = layout_state.as_ref().and_then(|layout_state| {
                self.geo_foundation_ui.selected_placement.as_ref()
                    .and_then(|id| layout_state.input.ground_links.get(id.as_str()))
            });
            self.geo_foundation_ui.layout_link_model = link.map(|link| link.model_id.clone());
            self.geo_foundation_ui.layout_link_scenario = link.and_then(|link| link.scenario_id.clone());
        }

        let placement_labels: Vec<String> = layout.as_ref().map(|layout| layout.elements.iter().map(|placement| {
            format!("{} · {}", foundation_placement_id(placement), foundation_placement_kind(placement))
        }).collect()).unwrap_or_default();
        self.ui.drop_down(cx, ids!(gf_placement_select)).set_labels(cx, selector_labels_or_empty(placement_labels, "No stored placements"));
        self.ui.drop_down(cx, ids!(gf_placement_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.placement_ids, self.geo_foundation_ui.selected_placement.as_ref()));
        self.ui.drop_down(cx, ids!(gf_tier_select)).set_labels(cx, selector_labels_or_empty(self.geo_foundation_ui.tier_ids.clone(), "No tiers"));
        self.ui.drop_down(cx, ids!(gf_tier_select)).set_selected_item(cx, selector_index(&self.geo_foundation_ui.tier_ids, self.geo_foundation_ui.selected_tier_id.as_ref()));

        self.geo_foundation_ui.ground_link_model_ids = state.records.models.keys().cloned().collect();
        let mut model_labels: Vec<String> = vec!["No model selected".to_owned()];
        model_labels.extend(state.records.models.iter().map(|(id, model)| format!("{} · {}", id, model.name)));
        self.ui.drop_down(cx, ids!(gf_ground_link_model)).set_labels(cx, model_labels);
        let model_index = self.geo_foundation_ui.layout_link_model.as_ref()
            .map_or(0, |id| selector_index(&self.geo_foundation_ui.ground_link_model_ids, Some(id)) + 1);
        self.ui.drop_down(cx, ids!(gf_ground_link_model)).set_selected_item(cx, model_index);
        let selected_model_id = self.geo_foundation_ui.layout_link_model.as_ref();
        self.geo_foundation_ui.ground_link_scenario_ids = selected_model_id.and_then(|model_id| {
            Some(state.records.scenarios.values().filter(|scenario| &scenario.model_id == model_id)
                .map(|scenario| scenario.id.clone()).collect::<Vec<_>>())
        }).unwrap_or_default();
        let mut scenario_labels: Vec<String> = vec!["No scenario selected".to_owned()];
        scenario_labels.extend(self.geo_foundation_ui.ground_link_scenario_ids.iter().filter_map(|id| {
            state.records.scenarios.get(id).map(|scenario| format!("{} · {}", id, scenario.name))
        }));
        self.ui.drop_down(cx, ids!(gf_ground_link_scenario)).set_labels(cx, scenario_labels);
        let scenario_index = self.geo_foundation_ui.layout_link_scenario.as_ref()
            .map_or(0, |id| selector_index(&self.geo_foundation_ui.ground_link_scenario_ids, Some(id)) + 1);
        self.ui.drop_down(cx, ids!(gf_ground_link_scenario)).set_selected_item(cx, scenario_index);

        let status = layout_state.as_ref().map_or_else(
            || "No saved foundation layout. Add a typed placement to create a layout draft.".to_owned(),
            |layout_state| format!("Layout {} · {} placement(s) · source operation {} · input {}. Geometry is not bearing/settlement acceptance.",
                layout_state.input.layout.id, layout_state.input.layout.elements.len(),
                layout_state.source_revision.operation_id.get(), layout_state.input_fingerprint),
        );
        self.ui.text_input(cx, ids!(gf_layout_status)).set_text(cx, &status);
        let mut foundation_snapshot_labels: Vec<String> = vec!["No analysis snapshot selected · result layers HOLD".to_owned()];
        foundation_snapshot_labels.extend(foundation_snapshot_items.iter().map(|(id, snapshot, status)| {
            format!("{} · {:?} · placement {} · {}", id.get(), status, snapshot.input.foundation_id, snapshot.content_fingerprint)
        }));
        self.ui.drop_down(cx, ids!(gf_foundation_snapshot_select)).set_labels(cx, foundation_snapshot_labels);
        let foundation_snapshot_index = self.geo_foundation_ui.selected_foundation_snapshot.as_ref()
            .and_then(|id| self.geo_foundation_ui.foundation_snapshot_ids.iter().position(|value| value == id))
            .map_or(0, |index| index + 1);
        self.ui.drop_down(cx, ids!(gf_foundation_snapshot_select)).set_selected_item(cx, foundation_snapshot_index);
        if !self.geo_foundation_ui.layout_dirty {
            let layout_id = self.geo_foundation_ui.layout_draft.as_ref().map_or_else(String::new, |layout| layout.id.clone());
            self.ui.text_input(cx, ids!(gf_layout_id)).set_text(cx, &layout_id);
            self.geo_foundation_load_layout_form(cx);
        }
    }

    fn geo_foundation_update_header(&mut self, cx: &mut Cx) {
        let binding = self.geo_foundation_ui.binding.as_ref().map_or_else(
            || "No source binding".to_owned(),
            |binding| format!("Document {} · GEO records {} · revision {}", binding.document_uuid, binding.records_fingerprint, binding.document_revision),
        );
        self.ui.label(cx, ids!(gf_binding_label)).set_text(cx, &binding);
        self.ui.label(cx, ids!(gf_status)).set_text(cx, &self.geo_foundation_ui.status);
        self.ui.text_input(cx, ids!(gf_query_text)).set_text(cx, &self.geo_foundation_ui.query_text);
        let snapshot_status = self.geo_foundation_ui.projection.as_ref().map_or_else(
            || "No current selected snapshot".to_owned(),
            |projection| match projection.snapshot_currentness {
                Some(geo_core::SnapshotCurrentness::Current) => {
                    if self.geo_foundation_ui.selected_snapshot.is_some() { "STORED SNAPSHOT · CURRENT".into() } else { "CURRENT-VIEW snapshot · read-only".into() }
                }
                Some(geo_core::SnapshotCurrentness::Stale) => "STALE SNAPSHOT · derived surfaces hidden".into(),
                None => "MODEL_INCOMPLETE · choose explicit model + water scenario".into(),
            },
        );
        self.ui.label(cx, ids!(gf_snapshot_status)).set_text(cx, &snapshot_status);
        self.ui.label(cx, ids!(gf_render_status)).set_text(cx, &self.geo_foundation_ui.status);
        let report = self.geo_foundation_ui.report_text.clone();
        self.ui.text_input(cx, ids!(gf_report_text)).set_text(cx, &report);
    }

    fn geo_foundation_set_input(&mut self, cx: &mut Cx, widget: &[LiveId], value: &str) {
        self.ui.text_input(cx, widget).set_text(cx, value);
    }

    fn geo_foundation_load_layout_form(&mut self, cx: &mut Cx) {
        let layout = self.geo_foundation_ui.layout_draft.clone();
        let layout_id = layout.as_ref().map_or_else(String::new, |layout| layout.id.clone());
        self.geo_foundation_set_input(cx, ids!(gf_layout_id), &layout_id);

        let selected = layout.as_ref().and_then(|layout| self.geo_foundation_ui.selected_placement.as_ref()
            .and_then(|id| layout.elements.iter().find(|placement| foundation_placement_id(placement) == id)));
        let Some(placement) = selected else {
            for widget in [
                ids!(gf_placement_id), ids!(gf_placement_x), ids!(gf_placement_y), ids!(gf_placement_z),
                ids!(gf_placement_length_x), ids!(gf_placement_width_y), ids!(gf_placement_thickness),
                ids!(gf_placement_strip_length), ids!(gf_placement_strip_width), ids!(gf_placement_rotation),
                ids!(gf_tier_id), ids!(gf_tier_x), ids!(gf_tier_y), ids!(gf_tier_z),
                ids!(gf_tier_length_x), ids!(gf_tier_width_y), ids!(gf_tier_thickness),
            ] {
                self.geo_foundation_set_input(cx, widget, "");
            }
            self.geo_foundation_load_bedding_fields(cx, None);
            self.geo_foundation_ui.layout_fields_dirty = false;
            return;
        };

        let placement_id = foundation_placement_id(placement);
        let is_new_placement = self.geo_foundation_ui.new_placement_ids.contains(placement_id);
        self.geo_foundation_set_input(cx, ids!(gf_placement_id), placement_id.as_str());
        let kind_index = match placement {
            FoundationPlacement::Raft { .. } => 0,
            FoundationPlacement::Strip { .. } => 1,
            FoundationPlacement::Isolated { .. } => 2,
            FoundationPlacement::Stepped { .. } => 3,
        };
        self.ui.drop_down(cx, ids!(gf_placement_kind)).set_selected_item(cx, kind_index);
        let (center, length_x, width_y, thickness, strip_length, strip_width, rotation) = match placement {
            FoundationPlacement::Raft { center_m, length_x_m, width_y_m, thickness_m, .. }
            | FoundationPlacement::Isolated { center_m, length_x_m, width_y_m, thickness_m, .. } =>
                (Some(*center_m), Some(*length_x_m), Some(*width_y_m), Some(*thickness_m), None, None, None),
            FoundationPlacement::Strip { center_m, length_m, width_m, rotation_rad, thickness_m, .. } =>
                (Some(*center_m), None, None, Some(*thickness_m), Some(*length_m), Some(*width_m), Some(*rotation_rad)),
            FoundationPlacement::Stepped { .. } => (None, None, None, None, None, None, None),
        };
        if is_new_placement {
            for widget in [ids!(gf_placement_x), ids!(gf_placement_y), ids!(gf_placement_z), ids!(gf_placement_length_x), ids!(gf_placement_width_y), ids!(gf_placement_thickness), ids!(gf_placement_strip_length), ids!(gf_placement_strip_width), ids!(gf_placement_rotation)] {
                self.geo_foundation_set_input(cx, widget, "");
            }
        } else {
            let coord = |index: usize| center.map_or_else(String::new, |point| point[index].to_string());
            self.geo_foundation_set_input(cx, ids!(gf_placement_x), &coord(0));
            self.geo_foundation_set_input(cx, ids!(gf_placement_y), &coord(1));
            self.geo_foundation_set_input(cx, ids!(gf_placement_z), &coord(2));
            self.geo_foundation_set_input(cx, ids!(gf_placement_length_x), &length_x.map_or_else(String::new, |value| value.to_string()));
            self.geo_foundation_set_input(cx, ids!(gf_placement_width_y), &width_y.map_or_else(String::new, |value| value.to_string()));
            self.geo_foundation_set_input(cx, ids!(gf_placement_thickness), &thickness.map_or_else(String::new, |value| value.to_string()));
            self.geo_foundation_set_input(cx, ids!(gf_placement_strip_length), &strip_length.map_or_else(String::new, |value| value.to_string()));
            self.geo_foundation_set_input(cx, ids!(gf_placement_strip_width), &strip_width.map_or_else(String::new, |value| value.to_string()));
            self.geo_foundation_set_input(cx, ids!(gf_placement_rotation), &rotation.map_or_else(String::new, |value| value.to_string()));
        }

        let tier = match placement {
            FoundationPlacement::Stepped { tiers, .. } => self.geo_foundation_ui.selected_tier_id.as_ref()
                .and_then(|id| tiers.iter().find(|tier| &tier.id == id)),
            _ => None,
        };
        if let Some(tier) = tier {
            self.geo_foundation_set_input(cx, ids!(gf_tier_id), &tier.id);
            if self.geo_foundation_ui.new_tier_ids.contains(&tier.id) {
                for widget in [ids!(gf_tier_x), ids!(gf_tier_y), ids!(gf_tier_z), ids!(gf_tier_length_x), ids!(gf_tier_width_y), ids!(gf_tier_thickness)] {
                    self.geo_foundation_set_input(cx, widget, "");
                }
            } else {
                self.geo_foundation_set_input(cx, ids!(gf_tier_x), &tier.center_m[0].to_string());
                self.geo_foundation_set_input(cx, ids!(gf_tier_y), &tier.center_m[1].to_string());
                self.geo_foundation_set_input(cx, ids!(gf_tier_z), &tier.center_m[2].to_string());
                self.geo_foundation_set_input(cx, ids!(gf_tier_length_x), &tier.length_x_m.to_string());
                self.geo_foundation_set_input(cx, ids!(gf_tier_width_y), &tier.width_y_m.to_string());
                self.geo_foundation_set_input(cx, ids!(gf_tier_thickness), &tier.thickness_m.to_string());
            }
        } else {
            for widget in [ids!(gf_tier_id), ids!(gf_tier_x), ids!(gf_tier_y), ids!(gf_tier_z), ids!(gf_tier_length_x), ids!(gf_tier_width_y), ids!(gf_tier_thickness)] {
                self.geo_foundation_set_input(cx, widget, "");
            }
        }
        self.geo_foundation_load_bedding_fields(cx, Some(placement_id));
        self.geo_foundation_ui.layout_fields_dirty = false;
    }

    fn geo_foundation_update_record_inspector(&mut self, cx: &mut Cx) {
        let layer_volume = match self.geo_foundation_ui.selection.as_ref() {
            Some(gf_render::GeoPick::LayerVolume { .. }) => {
                let current_key = self.geo_foundation_ui.projection.as_ref().and_then(|projection| {
                    projection.resolved.as_ref().map(|resolved| format!(
                        "{}|{:?}|{}",
                        resolved.snapshot.content_fingerprint,
                        projection.snapshot_currentness,
                        projection.snapshot_is_stored,
                    ))
                });
                self.geo_foundation_ui.selected_layer_volume_inspector.as_ref().and_then(|(fingerprint, text)| {
                    (current_key.as_deref() == Some(fingerprint.as_str())).then(|| text.clone())
                })
            }
            _ => None,
        };
        let ground_sample = match self.geo_foundation_ui.selection.as_ref() {
            Some(gf_render::GeoPick::FoundationGroundSample { .. }) => {
                self.geo_foundation_ui.selected_ground_sample_inspector.as_ref().and_then(|(key, text)| {
                    (self.geo_foundation_ui.ground_map_key.as_ref() == Some(key)).then(|| text.clone())
                })
            }
            _ => None,
        };
        let text = match self.geo_foundation_ui.selection.as_ref() {
            Some(gf_render::GeoPick::LayerVolume { .. }) => layer_volume.unwrap_or_else(||
                "LayerVolume source details are unavailable for the currently selected immutable snapshot; repick after the scene refreshes.".into()
            ),
            Some(gf_render::GeoPick::FoundationGroundSample { .. }) => ground_sample.unwrap_or_else(||
                "Ground-map sample details are unavailable because the cached map changed or was cleared; request a new map and pick a cell again.".into()
            ),
            _ => self.geo_foundation_record_json().unwrap_or_else(|| {
            "Select a stored Geo record to inspect its exact content and provenance.".into()
            }),
        };
        self.geo_foundation_ui.inspector_text = text.clone();
        self.ui.label(cx, ids!(gf_selection_label)).set_text(cx, &text);
        self.ui.text_input(cx, ids!(gf_inspector_text)).set_text(cx, &text);
    }

    fn geo_foundation_ground_sample_inspector(&self, foundation_id: &str, index: usize) -> Option<String> {
        let map = self.geo_foundation_ui.ground_map.as_ref()
            .filter(|map| map.foundation_id.as_str() == foundation_id)?;
        let cell = map.cells.get(index).filter(|cell| cell.index == index)?;
        let raw_source_query = match gf_api::ground_map_sample_query(map, index) {
            Ok(query) => serde_json::json!({
                "status": query.status,
                "triangle_ids": query.column.as_ref().map(|column| &column.triangle_ids),
                "column_source_ids": query.column.as_ref().map(|column| &column.source_ids),
                "source_ids": &query.source_ids,
                "result": query,
            }),
            Err(error) => serde_json::json!({ "status": "QUERY_HOLD", "message": error }),
        };
        let value = serde_json::json!({
            "selection": "Foundation midpoint sample",
            "foundation_id": foundation_id,
            "map_snapshot": {
                "selection_fingerprint": map.ground.selection_fingerprint,
                "content_fingerprint": map.ground.content_fingerprint,
                "source_operation_id": map.ground.source_revision.operation_id,
                "sampling_policy": map.sampling_policy,
            },
            "foundation_map_cell": cell,
            "foundation_map_cell_status": cell.status,
            "raw_source_query_at_same_midpoint": raw_source_query,
            "status_note": "Raw source-query status and foundation-sole map classification are distinct; the map may be Unknown because the sole is outside vertical coverage or lies on an unresolved interface.",
        });
        serde_json::to_string_pretty(&value).ok()
    }

    fn geo_foundation_layer_volume_inspector(&self, pick: &gf_render::GeoPick) -> Option<(String, String)> {
        let gf_render::GeoPick::LayerVolume { model_id, triangle_id, horizon_id, ige_id } = pick else { return None; };
        let projection = self.geo_foundation_ui.projection.as_ref()?;
        let resolved = projection.resolved.as_ref()?;
        let snapshot = &resolved.snapshot;
        let model = snapshot.records.models.get(model_id)?;
        let triangle = model.triangles.iter().find(|triangle| &triangle.id == triangle_id)?;
        let contact = model.contacts.iter().find(|contact| &contact.id == horizon_id)?;
        let ige = snapshot.records.iges.get(ige_id)?;
        let source_binding = self.geo_foundation_ui.scene_picks.iter()
            .find(|(_, scene_pick)| *scene_pick == pick)
            .and_then(|(object_id, _)| self.geo_foundation_ui.scene_sources.get(object_id))
            .filter(|sources| sources.geo_snapshot_id.as_ref() == Some(&snapshot.id)
                && sources.geo_source_revision.as_ref() == Some(&snapshot.source_revision));
        let attached_source_ids: BTreeSet<SourceId> = source_binding
            .map(|sources| sources.source_ids.clone())
            .unwrap_or_else(|| {
                triangle.source_ids.iter().chain(&contact.source_ids).chain(&ige.source_ids).cloned().collect()
            });
        let source_records: Vec<_> = attached_source_ids.iter()
            .filter_map(|source_id| snapshot.records.sources.get(source_id).map(|record| (source_id, record)))
            .collect();
        let value = serde_json::json!({
            "selection": "Core LayerVolume prism source records",
            "snapshot": {
                "selection_fingerprint": snapshot.selection_fingerprint,
                "content_fingerprint": snapshot.content_fingerprint,
                "source_revision": snapshot.source_revision,
                "currentness": projection.snapshot_currentness,
                "stored": projection.snapshot_is_stored,
            },
            "model": model,
            "triangle": triangle,
            "contact_horizon": contact,
            "ige": ige,
            "rendered_object_source_ids": attached_source_ids,
            "source_records_in_selected_immutable_snapshot": source_records,
            "status_note": "Triangle, contact, IGE, and source records are read from the selected immutable Geo snapshot. A stale snapshot remains identified as stale; it is not current engineering acceptance.",
        });
        let text = serde_json::to_string_pretty(&value).ok()?;
        let cache_key = format!("{}|{:?}|{}", snapshot.content_fingerprint, projection.snapshot_currentness, projection.snapshot_is_stored);
        Some((cache_key, text))
    }

    fn geo_foundation_record_json(&self) -> Option<String> {
        let projection = self.geo_foundation_ui.projection.as_ref()?;
        let state = &projection.state;
        let value = match self.geo_foundation_ui.active_tab {
            GeoFoundationTab::Sources => serde_json::to_value(state.records.sources.get(self.geo_foundation_ui.selected_source.as_ref()?)?).ok()?,
            GeoFoundationTab::Soils => serde_json::to_value(state.records.soils.get(self.geo_foundation_ui.selected_soil.as_ref()?)?).ok()?,
            GeoFoundationTab::Iges => serde_json::to_value(state.records.iges.get(self.geo_foundation_ui.selected_ige.as_ref()?)?).ok()?,
            GeoFoundationTab::Boreholes => serde_json::to_value(state.records.boreholes.get(self.geo_foundation_ui.selected_borehole.as_ref()?)?).ok()?,
            GeoFoundationTab::Model => serde_json::to_value(state.records.models.get(self.geo_foundation_ui.selected_model.as_ref()?)?).ok()?,
            GeoFoundationTab::Contacts => serde_json::to_value(state.records.models.get(self.geo_foundation_ui.selected_model.as_ref()?)?
                .contacts.iter().find(|contact| Some(&contact.id) == self.geo_foundation_ui.selected_contact.as_ref())?).ok()?,
            GeoFoundationTab::UnknownZones => serde_json::to_value(state.records.models.get(self.geo_foundation_ui.selected_model.as_ref()?)?
                .unknown_zones.get(self.geo_foundation_ui.unknown_zone_index?)?).ok()?,
            GeoFoundationTab::Water => {
                if !self.geo_foundation_ui.water_editing_observation {
                    let id = self.geo_foundation_ui.selected_scenario.as_ref()?;
                    serde_json::to_value(state.records.scenarios.get(id)?).ok()?
                } else {
                    serde_json::to_value(state.records.water_observations.get(self.geo_foundation_ui.selected_water_observation.as_ref()?)?).ok()?
                }
            }
            GeoFoundationTab::Sections => serde_json::to_value(state.records.sections.get(self.geo_foundation_ui.selected_section.as_ref()?)?).ok()?,
        };
        serde_json::to_string_pretty(&value).ok()
    }

    fn geo_foundation_clear_editor(&mut self, cx: &mut Cx) {
        for id in [
            id!(gf_source_id), id!(gf_source_title), id!(gf_source_reference), id!(gf_source_sha256),
            id!(gf_source_observed_at), id!(gf_soil_id), id!(gf_soil_name), id!(gf_parameter_value),
            id!(gf_parameter_missing_reason), id!(gf_parameter_source_ids), id!(gf_parameter_candidates),
            id!(gf_ige_id), id!(gf_ige_name), id!(gf_ige_source_ids), id!(gf_borehole_id),
            id!(gf_collar_x), id!(gf_collar_y), id!(gf_collar_z), id!(gf_borehole_collar_vertical_datum), id!(gf_borehole_source_ids),
            id!(gf_borehole_total_depth), id!(gf_borehole_total_depth_reason), id!(gf_borehole_total_depth_sources),
            id!(gf_borehole_date), id!(gf_borehole_crs), id!(gf_borehole_depth_datum),
            id!(gf_borehole_survey_sources), id!(gf_borehole_evidence_sources),
            id!(gf_interval_id), id!(gf_interval_top), id!(gf_interval_bottom), id!(gf_interval_source_ids),
            id!(gf_ground_model_id), id!(gf_ground_model_name), id!(gf_frame_name), id!(gf_frame_crs),
            id!(gf_frame_datum), id!(gf_model_source_ids), id!(gf_triangle_id), id!(gf_triangle_source_ids),
            id!(gf_contact_id), id!(gf_contact_vertical_datum), id!(gf_contact_elevation), id!(gf_contact_missing_reason),
            id!(gf_contact_source_ids), id!(gf_contact_candidates), id!(gf_unknown_polygon),
            id!(gf_unknown_reason), id!(gf_unknown_source_ids), id!(gf_scenario_id), id!(gf_scenario_name),
            id!(gf_scenario_source_ids), id!(gf_water_source_ids), id!(gf_water_observation_ids_text), id!(gf_water_observation_id), id!(gf_water_x), id!(gf_water_y),
            id!(gf_water_observed_at), id!(gf_water_elevation), id!(gf_water_elevation_source_ids),
            id!(gf_section_id), id!(gf_section_alignment), id!(gf_section_source_ids),
        ] {
            self.ui.text_input(cx, &[id]).set_text(cx, "");
        }
        self.geo_foundation_ui.draft = None;
    }

    fn geo_foundation_load_selected_editor(&mut self, cx: &mut Cx) {
        self.geo_foundation_clear_editor(cx);
        let Some(state) = self.geo_foundation_ui.projection.as_ref().map(|projection| projection.state.clone()) else {
            return;
        };
        match self.geo_foundation_ui.active_tab {
            GeoFoundationTab::Sources => if let Some(record) = self.geo_foundation_ui.selected_source.as_ref().and_then(|id| state.records.sources.get(id)) {
                self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Source(record.clone()));
                self.geo_foundation_set_input(cx, ids!(gf_source_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_source_title), &record.title);
                self.geo_foundation_set_input(cx, ids!(gf_source_reference), &record.reference);
                self.geo_foundation_set_input(cx, ids!(gf_source_sha256), record.content_sha256.as_deref().unwrap_or(""));
                self.geo_foundation_set_input(cx, ids!(gf_source_observed_at), record.observed_at.as_deref().unwrap_or(""));
                self.ui.drop_down(cx, ids!(gf_source_origin)).set_selected_item(cx, match record.origin {
                    geo_core::SourceOrigin::Synthetic => 1,
                    geo_core::SourceOrigin::Measured => 2,
                    geo_core::SourceOrigin::Interpreted => 3,
                    geo_core::SourceOrigin::ImportedLegacy => 4,
                });
            },
            GeoFoundationTab::Soils => if let Some(record) = self.geo_foundation_ui.selected_soil.as_ref().and_then(|id| state.records.soils.get(id)) {
                self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Soil(record.clone()));
                self.geo_foundation_set_input(cx, ids!(gf_soil_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_soil_name), &record.name);
                let index = self.ui.drop_down(cx, ids!(gf_soil_parameter)).selected_item().min(4);
                let parameter = soil_parameter_at(&record.parameters, index);
                let (status, value, reason, sources, candidates) = parameter_editor_fields(parameter);
                self.ui.drop_down(cx, ids!(gf_parameter_state)).set_selected_item(cx, status);
                self.geo_foundation_set_input(cx, ids!(gf_parameter_value), &value);
                self.geo_foundation_set_input(cx, ids!(gf_parameter_missing_reason), &reason);
                self.geo_foundation_set_input(cx, ids!(gf_parameter_source_ids), &sources);
                self.geo_foundation_set_input(cx, ids!(gf_parameter_candidates), &candidates);
                self.geo_foundation_set_input(cx, ids!(gf_parameter_status_summary), &serde_json::to_string_pretty(parameter).unwrap_or_default());
            },
            GeoFoundationTab::Iges => if let Some(record) = self.geo_foundation_ui.selected_ige.as_ref().and_then(|id| state.records.iges.get(id)) {
                self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Ige(record.clone()));
                self.geo_foundation_set_input(cx, ids!(gf_ige_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_ige_name), &record.name);
                self.geo_foundation_set_input(cx, ids!(gf_ige_source_ids), &source_ids_text(&record.source_ids));
                if let Some(index) = self.geo_foundation_ui.soil_ids.iter().position(|id| id == &record.soil_id) {
                    self.ui.drop_down(cx, ids!(gf_ige_soil)).set_selected_item(cx, index);
                }
            },
            GeoFoundationTab::Boreholes => if let Some(record) = self.geo_foundation_ui.selected_borehole.as_ref().and_then(|id| state.records.boreholes.get(id)) {
                self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Borehole(record.clone()));
                self.geo_foundation_set_input(cx, ids!(gf_borehole_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_collar_x), &record.collar_m[0].to_string());
                self.geo_foundation_set_input(cx, ids!(gf_collar_y), &record.collar_m[1].to_string());
                self.geo_foundation_set_input(cx, ids!(gf_collar_z), &record.collar_m[2].to_string());
                self.geo_foundation_set_input(cx, ids!(gf_borehole_collar_vertical_datum), record.collar_vertical_datum.as_deref().unwrap_or(""));
                self.geo_foundation_set_input(cx, ids!(gf_borehole_source_ids), &source_ids_text(&record.source_ids));
                let (status, value, reason, sources, _candidates) = parameter_editor_fields(&record.total_depth_m);
                self.ui.drop_down(cx, ids!(gf_borehole_total_depth_state)).set_selected_item(cx, status);
                self.geo_foundation_set_input(cx, ids!(gf_borehole_total_depth), &value);
                self.geo_foundation_set_input(cx, ids!(gf_borehole_total_depth_reason), &reason);
                self.geo_foundation_set_input(cx, ids!(gf_borehole_total_depth_sources), &sources);
                self.geo_foundation_set_input(cx, ids!(gf_borehole_date), record.drilling_date.as_deref().unwrap_or(""));
                self.geo_foundation_set_input(cx, ids!(gf_borehole_crs), record.crs.as_deref().unwrap_or(""));
                self.geo_foundation_set_input(cx, ids!(gf_borehole_depth_datum), &record.depth_datum);
                self.geo_foundation_set_input(cx, ids!(gf_borehole_survey_sources), &source_ids_text(&record.survey_source_ids));
                self.geo_foundation_set_input(cx, ids!(gf_borehole_evidence_sources), &source_ids_text(&record.evidence_source_ids));
                let rows = record.intervals.iter().map(|interval| format!("{} · {:.3}–{:.3} m · IGE {} · sources [{}]", interval.id, interval.top_depth_m, interval.bottom_depth_m, interval.ige_id, source_ids_text(&interval.source_ids))).collect::<Vec<_>>().join("\n");
                self.geo_foundation_set_input(cx, ids!(gf_interval_rows), &rows);
                if let Some(interval) = self.geo_foundation_ui.selected_interval.as_ref().and_then(|id| record.intervals.iter().find(|interval| &interval.id == id)) {
                    self.geo_foundation_set_input(cx, ids!(gf_interval_top), &interval.top_depth_m.to_string());
                    self.geo_foundation_set_input(cx, ids!(gf_interval_bottom), &interval.bottom_depth_m.to_string());
                    self.geo_foundation_set_input(cx, ids!(gf_interval_source_ids), &source_ids_text(&interval.source_ids));
                    if let Some(index) = self.geo_foundation_ui.ige_ids.iter().position(|id| id == &interval.ige_id) {
                        self.ui.drop_down(cx, ids!(gf_interval_ige)).set_selected_item(cx, index);
                    }
                }
            },
            GeoFoundationTab::Model => if let Some(record) = self.geo_foundation_ui.selected_model.as_ref().and_then(|id| state.records.models.get(id)) {
                self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Model(record.clone()));
                self.geo_foundation_set_input(cx, ids!(gf_ground_model_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_ground_model_name), &record.name);
                self.geo_foundation_set_input(cx, ids!(gf_frame_name), &record.frame.name);
                self.geo_foundation_set_input(cx, ids!(gf_frame_crs), record.frame.crs.as_deref().unwrap_or(""));
                self.geo_foundation_set_input(cx, ids!(gf_frame_datum), record.frame.vertical_datum.as_deref().unwrap_or(""));
                self.geo_foundation_set_input(cx, ids!(gf_model_source_ids), &source_ids_text(&record.source_ids));
                self.geo_foundation_set_input(cx, ids!(gf_model_boreholes), &record.borehole_ids.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"));
                self.ui.check_box(cx, ids!(gf_allow_pinch)).set_active(cx, record.allow_pinch, Animate::No);
                if let Some(triangle) = self.geo_foundation_ui.selected_triangle.as_ref().and_then(|id| record.triangles.iter().find(|triangle| &triangle.id == id)) {
                    self.geo_foundation_set_input(cx, ids!(gf_triangle_id), triangle.id.as_str());
                    self.geo_foundation_set_input(cx, ids!(gf_triangle_source_ids), &source_ids_text(&triangle.source_ids));
                    for (widget, vertex) in [(ids!(gf_triangle_vertex_a), &triangle.vertices[0]), (ids!(gf_triangle_vertex_b), &triangle.vertices[1]), (ids!(gf_triangle_vertex_c), &triangle.vertices[2])] {
                        if let Some(index) = self.geo_foundation_ui.model_borehole_ids.iter().position(|id| id == vertex) {
                            self.ui.drop_down(cx, widget).set_selected_item(cx, index);
                        }
                    }
                }
            },
            GeoFoundationTab::Contacts => if let (Some(model_id), Some(contact_id)) = (self.geo_foundation_ui.selected_model.as_ref(), self.geo_foundation_ui.selected_contact.as_ref()) {
                if let Some(contact) = state.records.models.get(model_id).and_then(|model| model.contacts.iter().find(|contact| &contact.id == contact_id)) {
                    self.geo_foundation_set_input(cx, ids!(gf_contact_id), contact.id.as_str());
                    self.geo_foundation_set_input(cx, ids!(gf_contact_vertical_datum), contact.vertical_datum.as_deref().unwrap_or(""));
                    self.geo_foundation_set_input(cx, ids!(gf_contact_source_ids), &source_ids_text(&contact.source_ids));
                    if let Some(index) = self.geo_foundation_ui.ige_ids.iter().position(|id| id == &contact.ige_id) {
                        self.ui.drop_down(cx, ids!(gf_contact_ige)).set_selected_item(cx, index);
                    }
                    if self.geo_foundation_ui.selected_contact_borehole.is_none() {
                        self.geo_foundation_ui.selected_contact_borehole = contact.elevations_abs_m.keys().next().cloned()
                            .or_else(|| self.geo_foundation_ui.model_borehole_ids.first().cloned());
                    }
                    if let Some(borehole_id) = self.geo_foundation_ui.selected_contact_borehole.as_ref() {
                        if let Some(index) = self.geo_foundation_ui.model_borehole_ids.iter().position(|id| id == borehole_id) {
                            self.ui.drop_down(cx, ids!(gf_contact_borehole)).set_selected_item(cx, index);
                        }
                        if let Some(parameter) = contact.elevations_abs_m.get(borehole_id) {
                            let (status, value, reason, sources, candidates) = parameter_editor_fields(parameter);
                            self.ui.drop_down(cx, ids!(gf_contact_parameter_state)).set_selected_item(cx, status);
                            self.geo_foundation_set_input(cx, ids!(gf_contact_elevation), &value);
                            self.geo_foundation_set_input(cx, ids!(gf_contact_missing_reason), &reason);
                            self.geo_foundation_set_input(cx, ids!(gf_contact_source_ids), &sources);
                            self.geo_foundation_set_input(cx, ids!(gf_contact_candidates), &candidates);
                            self.geo_foundation_set_input(cx, ids!(gf_contact_summary), &serde_json::to_string_pretty(&contact.elevations_abs_m).unwrap_or_default());
                        }
                    }
                }
            },
            GeoFoundationTab::UnknownZones => if let Some(zone) = self.geo_foundation_ui.selected_model.as_ref().and_then(|id| state.records.models.get(id))
                .and_then(|model| self.geo_foundation_ui.unknown_zone_index.and_then(|index| model.unknown_zones.get(index))) {
                self.geo_foundation_ui.draft = Some(GeoFoundationDraft::UnknownZone(zone.clone()));
                self.geo_foundation_set_input(cx, ids!(gf_unknown_polygon), &zone.polygon_xy_m.iter().map(|point| format!("{}, {}", point[0], point[1])).collect::<Vec<_>>().join("\n"));
                self.geo_foundation_set_input(cx, ids!(gf_unknown_reason), &zone.reason);
                self.geo_foundation_set_input(cx, ids!(gf_unknown_source_ids), &source_ids_text(&zone.source_ids));
            },
            GeoFoundationTab::Water => {
                if !self.geo_foundation_ui.water_editing_observation {
                    if let Some(record) = self.geo_foundation_ui.selected_scenario.as_ref().and_then(|id| state.records.scenarios.get(id)) {
                    self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Scenario(record.clone()));
                    self.geo_foundation_set_input(cx, ids!(gf_scenario_id), record.id.as_str());
                    self.geo_foundation_set_input(cx, ids!(gf_scenario_name), &record.name);
                    self.geo_foundation_set_input(cx, ids!(gf_scenario_source_ids), &source_ids_text(&record.source_ids));
                    if let Some(index) = self.geo_foundation_ui.model_ids.iter().position(|id| id == &record.model_id) {
                        self.ui.drop_down(cx, ids!(gf_scenario_model)).set_selected_item(cx, index);
                    }
                    match &record.water {
                        geo_core::WaterScenario::Unknown => self.ui.drop_down(cx, ids!(gf_water_kind)).set_selected_item(cx, 0),
                        geo_core::WaterScenario::ExplicitDry { source_ids } => {
                            self.ui.drop_down(cx, ids!(gf_water_kind)).set_selected_item(cx, 1);
                            self.geo_foundation_set_input(cx, ids!(gf_water_source_ids), &source_ids_text(source_ids));
                        }
                        geo_core::WaterScenario::Observations { observation_ids } => {
                            self.ui.drop_down(cx, ids!(gf_water_kind)).set_selected_item(cx, 2);
                            self.geo_foundation_set_input(cx, ids!(gf_water_observation_ids_text), &observation_ids.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"));
                        }
                    }
                }
                } else if let Some(record) = self.geo_foundation_ui.selected_water_observation.as_ref().and_then(|id| state.records.water_observations.get(id)) {
                    self.geo_foundation_ui.draft = Some(GeoFoundationDraft::WaterObservation(record.clone()));
                    self.geo_foundation_set_input(cx, ids!(gf_water_observation_id), record.id.as_str());
                    self.geo_foundation_set_input(cx, ids!(gf_water_x), &record.xy_m[0].to_string());
                    self.geo_foundation_set_input(cx, ids!(gf_water_y), &record.xy_m[1].to_string());
                    self.geo_foundation_set_input(cx, ids!(gf_water_observed_at), record.observed_at.as_deref().unwrap_or(""));
                    let (status, value, _reason, sources, _candidates) = parameter_editor_fields(&record.elevation_abs_m);
                    self.ui.drop_down(cx, ids!(gf_water_elevation_state)).set_selected_item(cx, status);
                    self.geo_foundation_set_input(cx, ids!(gf_water_elevation), &value);
                    self.geo_foundation_set_input(cx, ids!(gf_water_elevation_source_ids), &sources);
                }
            }
            GeoFoundationTab::Sections => if let Some(record) = self.geo_foundation_ui.selected_section.as_ref().and_then(|id| state.records.sections.get(id)) {
                self.geo_foundation_ui.draft = Some(GeoFoundationDraft::Section(record.clone()));
                self.geo_foundation_set_input(cx, ids!(gf_section_id), record.id.as_str());
                self.geo_foundation_set_input(cx, ids!(gf_section_alignment), &record.polyline_xy_m.iter().map(|point| format!("{}, {}", point[0], point[1])).collect::<Vec<_>>().join("\n"));
                self.geo_foundation_set_input(cx, ids!(gf_section_source_ids), &source_ids_text(&record.source_ids));
                if let Some(index) = self.geo_foundation_ui.model_ids.iter().position(|id| id == &record.model_id) {
                    self.ui.drop_down(cx, ids!(gf_section_model)).set_selected_item(cx, index);
                }
            },
        }
        self.geo_foundation_ui.draft_dirty = false;
        self.geo_foundation_ui.binding_stale = false;
    }

    fn geo_foundation_clear_scene(&mut self, cx: &mut Cx) {
        let canvas_needs_reset = self.ui.drafting_canvas(cx, ids!(gf_canvas)).borrow().is_some_and(|canvas| {
            let frame = &canvas.frame;
            !frame.segments.is_empty()
                || !frame.highlights.is_empty()
                || !frame.preview.is_empty()
                || frame.crosshair.is_some()
                || !frame.analysis.segments.is_empty()
                || !frame.analysis.triangles.is_empty()
                || !frame.analysis.labels.is_empty()
                || !frame.snapshot_token.is_empty()
                || frame.source_generation != 0
                || frame.model_generation != 0
                || frame.document_revision != 0
                || frame.camera_revision != 0
                || frame.camera_change.is_some()
                || frame.cpu_frame_prepare_ms.is_some()
        });
        let retained_screen_has_visuals = !self.geo_foundation_ui.screen.segments.is_empty()
            || !self.geo_foundation_ui.screen.triangles.is_empty()
            || !self.geo_foundation_ui.screen.labels.is_empty();
        let already_empty = self.geo_foundation_ui.scene_key.is_none()
            && self.geo_foundation_ui.screen_key.is_none()
            && self.geo_foundation_ui.scene_picks.is_empty()
            && self.geo_foundation_ui.scene_sources.is_empty()
            && self.ui.cad_viewport(cx, ids!(gf_viewport)).borrow()
                .is_none_or(|viewport| viewport.viewer().scene().objects().next().is_none());
        if already_empty {
            if retained_screen_has_visuals {
                self.geo_foundation_ui.screen = Arc::new(AnalysisCanvasLayer::default());
            }
            if canvas_needs_reset {
                let canvas_ref = self.ui.drafting_canvas(cx, ids!(gf_canvas));
                if let Some(mut canvas) = canvas_ref.borrow_mut() { canvas.set_frame(cx, CanvasFrame::default()); }
            }
            self.ui.widget(cx, ids!(gf_canvas_host)).set_visible(cx, true);
            self.ui.widget(cx, ids!(gf_scene_host)).set_visible(cx, false);
            return;
        }
        self.geo_foundation_ui.scene_generation = self.geo_foundation_ui.scene_generation.saturating_add(1);
        self.geo_foundation_ui.scene_picks.clear();
        self.geo_foundation_ui.scene_sources.clear();
        self.geo_foundation_ui.scene_source_fingerprint = None;
        self.geo_foundation_ui.scene_foundation_snapshot_fingerprint = None;
        self.geo_foundation_ui.scene_key = None;
        self.geo_foundation_ui.screen_key = None;
        self.geo_foundation_ui.selection = None;
        self.geo_foundation_ui.screen = Arc::new(AnalysisCanvasLayer::default());
        let canvas_ref = self.ui.drafting_canvas(cx, ids!(gf_canvas));
        if let Some(mut canvas) = canvas_ref.borrow_mut() { canvas.set_frame(cx, CanvasFrame::default()); }
        let viewport_ref = self.ui.cad_viewport(cx, ids!(gf_viewport));
        let old_state = viewport_ref.borrow().map(|viewport| viewport.viewer().capture_state());
        let mut empty_viewer = Viewer::default();
        if let Some(mut state) = old_state {
            state.hidden_object_ids.clear();
            state.isolated_object_ids.clear();
            state.selected_object_ids.clear();
            state.highlighted_object_ids.clear();
            state.highlighted_object_id = None;
            state.navigation_pivot.object_id = None;
            state.hover_hit = None;
            let _ = empty_viewer.restore_state(&state);
        }
        viewport_ref.set_viewer(cx, empty_viewer);
        self.ui.widget(cx, ids!(gf_canvas_host)).set_visible(cx, true);
        self.ui.widget(cx, ids!(gf_scene_host)).set_visible(cx, false);
        self.ui.label(cx, ids!(gf_render_status)).set_text(cx, "MODEL_INCOMPLETE · no current typed render input");
    }

    fn geo_foundation_set_tab_host(&self, cx: &mut Cx) {
        for (tab, host) in [
            (GeoFoundationTab::Sources, ids!(gf_sources_host)),
            (GeoFoundationTab::Soils, ids!(gf_soils_host)),
            (GeoFoundationTab::Iges, ids!(gf_iges_host)),
            (GeoFoundationTab::Boreholes, ids!(gf_boreholes_host)),
            (GeoFoundationTab::Model, ids!(gf_model_host)),
            (GeoFoundationTab::Contacts, ids!(gf_contacts_host)),
            (GeoFoundationTab::Water, ids!(gf_water_host)),
            (GeoFoundationTab::Sections, ids!(gf_sections_host)),
            (GeoFoundationTab::UnknownZones, ids!(gf_unknown_host)),
        ] {
            self.ui.widget(cx, host).set_visible(cx, tab == self.geo_foundation_ui.active_tab);
        }
        self.ui.widget(cx, ids!(gf_foundation_host)).set_visible(cx, self.geo_foundation_ui.active_tab == GeoFoundationTab::Model);
    }
}

fn geo_foundation_query_identity(projection: &gf_api::GfProjection) -> String {
    // Query output is tied to the selected closure revisions and Document
    // instance. Deliberately omit global Document revision, source operation
    // and content fingerprint: the read-only current-view fingerprint embeds
    // the global operation even when an unrelated record changes.
    let closure = projection.selected_snapshot.as_ref().map(|snapshot| format!(
        "{}|{}|{}",
        snapshot.selection_fingerprint,
        snapshot.selection.model_id,
        snapshot.selection.scenario_id.as_ref().map(ToString::to_string).unwrap_or_else(|| "no-scenario".into()),
    )).unwrap_or_else(|| "no-selected-closure".into());
    let origin = projection.selected_snapshot.as_ref().map(|snapshot| {
        if projection.snapshot_is_stored { format!("stored:{}", snapshot.id) } else { "current-view".to_owned() }
    }).unwrap_or_else(|| "no-snapshot".into());
    let resolved = projection.resolved.as_ref().map(|resolved| resolved.scenario.id.to_string())
        .unwrap_or_else(|| "unresolved".into());
    format!("{}|{}|{}|{}|{}|{:?}|{}|{}",
        projection.binding.document_uuid,
        projection.binding.runtime_id,
        projection.binding.open_generation,
        closure,
        origin,
        projection.snapshot_currentness,
        resolved,
        projection.status,
    )
}

pub(super) fn parse_finite_field(text: &str, label: &str) -> Result<f64, String> {
    let value = text
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{label}: enter a finite number in the displayed units"))?;
    if !value.is_finite() {
        return Err(format!("{label}: value must be finite"));
    }
    Ok(value)
}

pub(super) fn parse_source_ids(text: &str) -> Result<Vec<SourceId>, String> {
    let mut seen = BTreeSet::new();
    let mut ids = Vec::new();
    for value in text
        .split(|character: char| character == ',' || character == ';' || character == '\n')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let id = SourceId::new(value.to_owned()).map_err(|error| format!("source ID: {error}"))?;
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate source ID: {id}"));
        }
        ids.push(id);
    }
    Ok(ids)
}

pub(super) fn parse_geo_id<T>(text: &str, label: &str) -> Result<T, String>
where
    T: TryFrom<String>,
    <T as TryFrom<String>>::Error: std::fmt::Display,
{
    T::try_from(text.trim().to_owned()).map_err(|error| format!("{label}: {error}"))
}

fn ensure_record_id<T: PartialEq>(stored: &T, edited: &T, is_new: bool, label: &str) -> Result<(), String> {
    if !is_new && stored != edited {
        return Err(format!("{label} ID is immutable; create a new record to use another ID."));
    }
    Ok(())
}

fn optional_field(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_id_lines<T>(text: &str, label: &str) -> Result<Vec<T>, String>
where
    T: TryFrom<String> + PartialEq,
    <T as TryFrom<String>>::Error: std::fmt::Display,
{
    let mut ids = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let id = parse_geo_id::<T>(line, label)
            .map_err(|error| format!("line {}: {error}", line_index + 1))?;
        if ids.contains(&id) {
            return Err(format!("line {}: duplicate {label}", line_index + 1));
        }
        ids.push(id);
    }
    Ok(ids)
}

pub(super) fn parse_xy_pairs(text: &str, label: &str) -> Result<Vec<[f64; 2]>, String> {
    let mut points = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<_> = line
            .split(|character: char| character == ',' || character == ';' || character.is_whitespace())
            .filter(|part| !part.is_empty())
            .collect();
        if parts.len() != 2 {
            return Err(format!("{label}, line {}: enter exactly X and Y", line_index + 1));
        }
        points.push([
            parse_finite_field(parts[0], &format!("{label} X"))?,
            parse_finite_field(parts[1], &format!("{label} Y"))?,
        ]);
    }
    Ok(points)
}

pub(super) fn source_ids_text(ids: &[SourceId]) -> String {
    ids.iter().map(SourceId::as_str).collect::<Vec<_>>().join("\n")
}

pub(super) fn parameter_status_name<T>(parameter: &Parameter<T>) -> &'static str {
    match parameter {
        Parameter::Missing { .. } => "MISSING",
        Parameter::Known { .. } => "KNOWN · not reviewed",
        Parameter::Conflicting { .. } => "CONFLICTING",
    }
}
