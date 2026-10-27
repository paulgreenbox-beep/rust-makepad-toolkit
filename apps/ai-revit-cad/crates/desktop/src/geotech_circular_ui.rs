//! Native circular-slope draft and saved-evidence presentation adapter.
//! No Document, worker, solver execution or history mutation lives here.
//! CHEAPEST_VERIFICATION_PATH: focused lifecycle tests, then Root-owned consumer
//! verification and native Open-before-Analyze acceptance; not a unit-only gate.
use crate::geotech_circular_render::OverlayFlags;
use crate::geotech_circular_archive_view::{ArchiveInspection, ArchiveState};
use ai_revit_core::{
    CircularSearchSpecV1, CommandId, CommitReceipt, OperationId, ProjectObjectRef,
    ResolvedCircularSearch,
    engineering::{
        AnalysisCaseId, CircularAnalysisCase, CircularCaseDefinition, EngineeringCommand,
        GroundSection, GroundSectionId as DocumentGroundId, ResolvedCircularAnalysis,
    },
};
use geotech_core::{InputUnits, Point2, SoilMaterial, circular::*};
use makepad_widgets::*;
use serde::Serialize;
use std::{collections::BTreeSet, fmt::Write, sync::Arc};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    let Field = TextInput{width: Fill height: 30 empty_text: "Обязательно"}
    let Caption = Label{width: Fill draw_text.color: #xb7cadb}
    mod.widgets.GeotechCircularParameters = SolidView{
        width: Fill height: Fill flow: Down padding: 12 spacing: 6
        draw_bg.color: #x17212b
        H3{width: Fill text: "Самостоятельный разрез грунта"}
        gcc_binding := Label{width: Fill text: "Нет привязки к Document" draw_text.color: #xf2c47c}
        gcc_ground_selector := DropDown{width: Fill labels: ["Выберите сохранённый разрез"]}
        View{width: Fill height: 32 flow: Right spacing: 6
            gcc_new := Button{width: Fill text: "Новый разрез"}
            gcc_reload := Button{width: Fill text: "Загрузить разрез"}
        }
        ScrollYView{width: Fill height: Fill flow: Down spacing: 6
            gcc_ground_id := Label{width: Fill text: "ID разреза назначает Document"}
            gcc_ground_name := Field{empty_text: "Название разреза"}
            gcc_readiness := Label{width: Fill text: "Готовность разреза не определена"}
            gcc_preview_width := Field{empty_text: "Ширина 3D-предпросмотра, м · только вид"}
            Caption{text: "Рельеф / подошва / граница · X,Z в метрах"}
            gcc_point_boundary := DropDown{width: Fill labels: ["Рельеф", "Подошва", "Выбранная граница"]}
            gcc_interface_selector := DropDown{width: Fill labels: ["Нет границ"]}
            gcc_interface_id := Field{empty_text: "ID границы"}
            View{width: Fill height: 32 flow: Right spacing: 4
                gcc_interface_add := Button{width: Fill text: "+ граница"}
                gcc_interface_remove := Button{width: Fill text: "Удалить границу"}
            }
            gcc_point_index := Field{empty_text: "Индекс точки · с 0"}
            View{width: Fill height: 30 flow: Right spacing: 6
                gcc_point_x := Field{empty_text: "X, м"}
                gcc_point_z := Field{empty_text: "Z, м · вверх"}
            }
            View{width: Fill height: 32 flow: Right spacing: 4
                gcc_point_load := Button{width: Fill text: "Читать точку"}
                gcc_point_add := Button{width: Fill text: "Вставить"}
                gcc_point_apply := Button{width: Fill text: "Применить"}
                gcc_point_remove := Button{width: Fill text: "Удалить"}
            }
            gcc_geometry_rows := TextInput{width: Fill height: 130 is_multiline: true is_read_only: true text: "Нет точек"}
            Hr{}
            Caption{text: "Материалы · без скрытых значений свойств"}
            gcc_material_selector := DropDown{width: Fill labels: ["Новый материал"]}
            gcc_material_id := Field{empty_text: "Material ID"}
            gcc_material_name := Field{empty_text: "Название грунта"}
            Caption{text: "Удельный вес γ, кН/м³ · не плотность"}
            gcc_gamma := Field{empty_text: "γ > 0, кН/м³"}
            Caption{text: "Эффективные c′, кПа; φ′, градусы"}
            View{width: Fill height: 30 flow: Right spacing: 6
                gcc_cohesion := Field{empty_text: "c′ ≥ 0, кПа"}
                gcc_phi := Field{empty_text: "0 ≤ φ′ < 90°"}
            }
            gcc_provenance := TextInput{width: Fill height: 60 is_multiline: true empty_text: "Источник свойств · обязательно"}
            View{width: Fill height: 32 flow: Right spacing: 5
                gcc_material_apply := Button{width: Fill text: "Добавить / применить"}
                gcc_material_remove := Button{width: Fill text: "Удалить"}
            }
            Caption{text: "Области сверху вниз: terrain / interfaces / bottom"}
            gcc_region_index := Field{empty_text: "Индекс области · с 0"}
            gcc_region_id := Field{empty_text: "Region ID"}
            gcc_region_material := Field{empty_text: "Назначенный Material ID"}
            View{width: Fill height: 32 flow: Right spacing: 5
                gcc_region_load := Button{width: Fill text: "Читать"}
                gcc_region_apply := Button{width: Fill text: "Применить"}
                gcc_region_remove := Button{width: Fill text: "Удалить"}
            }
            gcc_region_rows := TextInput{width: Fill height: 95 is_multiline: true is_read_only: true text: "Нет областей"}
            Hr{}
            H3{width: Fill text: "Отдельный расчётный случай"}
            gcc_case_selector := DropDown{width: Fill labels: ["Выберите сохранённый case"]}
            View{width: Fill height: 32 flow: Right spacing: 5
                gcc_case_new := Button{width: Fill text: "Новый case"}
                gcc_case_reload := Button{width: Fill text: "Загрузить case"}
            }
            gcc_analysis_id := Label{width: Fill text: "ID case назначает Document"}
            gcc_case_name := Field{empty_text: "Название case"}
            gcc_case_ground := DropDown{width: Fill labels: ["Ссылка на существующий разрез"]}
            gcc_case_sources := Label{width: Fill text: "Ground/case source stamps не загружены"}
            gcc_full_config := TextInput{width: Fill height: 130 is_multiline: true is_read_only: true text: "Полный config не загружен"}
            gcc_config_edit := Button{width: Fill text: "Все настройки расчёта"}
            gcc_slip_id := Field{empty_text: "Локальный ID поверхности скольжения"}
            Caption{text: "Круговая поверхность · только нижняя дуга"}
            gcc_circle_mode := DropDown{width: Fill labels: ["Центр + радиус", "Три точки нижней дуги"]}
            gcc_direction := DropDown{width: Fill labels: ["Скольжение +X", "Скольжение −X"]}
            View{width: Fill height: 30 flow: Right spacing: 6
                gcc_center_x := Field{empty_text: "X центра, м"}
                gcc_center_z := Field{empty_text: "Z центра, м"}
            }
            gcc_radius := Field{empty_text: "Радиус > 0, м"}
            View{width: Fill height: 30 flow: Right spacing: 6
                gcc_p1x := Field{empty_text: "Точка 1 · X, м"}
                gcc_p1z := Field{empty_text: "Точка 1 · Z, м"}
            }
            View{width: Fill height: 30 flow: Right spacing: 6
                gcc_p2x := Field{empty_text: "Точка 2 · X, м"}
                gcc_p2z := Field{empty_text: "Точка 2 · Z, м"}
            }
            View{width: Fill height: 30 flow: Right spacing: 6
                gcc_p3x := Field{empty_text: "Точка 3 · X, м"}
                gcc_p3z := Field{empty_text: "Точка 3 · Z, м"}
            }
            gcc_circle_preview := Button{width: Fill text: "Предпросмотр окружности · без Fs"}
            Caption{text: "Разбиение и 3D-визуализация"}
            gcc_slice_count := Field{empty_text: "Число отсеков"}
            gcc_refine := CheckBox{text: "Проверить сгущение разбиения"}
            gcc_width := Field{empty_text: "Ширина 3D-экструзии B, м"}
            Caption{text: "Поиск · включённые границы, м; число интервалов"}
            gcc_search_enabled := CheckBox{text: "Включить локальный предпросмотр поиска"}
            Caption{text: "Предпросмотр и фиксация действуют в текущем сеансе. Сохранение настроек записывает политику в case."}
            View{width: Fill height: 32 flow: Right spacing: 5
                gcc_search_policy := Button{width: Fill text: "Базовые настройки поиска"}
                gcc_search_save := Button{width: Fill text: "Сохранить настройки поиска"}
            }
            gcc_search_freeze := Button{width: Fill text: "Зафиксировать параметры поиска"}
            View{width: Fill height: 30 flow: Right spacing: 5
                gcc_xmin := Field{empty_text: "X мин"} gcc_xmax := Field{empty_text: "X макс"} gcc_xintervals := Field{empty_text: "Интервалов X"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                gcc_zmin := Field{empty_text: "Z мин"} gcc_zmax := Field{empty_text: "Z макс"} gcc_zintervals := Field{empty_text: "Интервалов Z"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                gcc_rmin := Field{empty_text: "R мин > 0"} gcc_rmax := Field{empty_text: "R макс"} gcc_rintervals := Field{empty_text: "Интервалов R"}
            }
            gcc_candidates_limit := Field{empty_text: "Лимит кандидатов · обязательно"}
            gcc_time_limit := Field{empty_text: "Лимит времени · 1..600000 мс"}
            Caption{text: "Допустимые вход/выход · закрытые X-диапазоны, м"}
            View{width: Fill height: 30 flow: Right spacing: 5
                gcc_entry_xmin := Field{empty_text: "Вход X мин"} gcc_entry_xmax := Field{empty_text: "Вход X макс"}
            }
            View{width: Fill height: 30 flow: Right spacing: 5
                gcc_exit_xmin := Field{empty_text: "Выход X мин"} gcc_exit_xmax := Field{empty_text: "Выход X макс"}
            }
            gcc_search_target := DropDown{width: Fill labels: ["Выберите целевой метод", "Fellenius", "Bishop", "Оба метода"]}
            gcc_search_levels := Field{empty_text: "Уровни локального уточнения · 0..8"}
            gcc_search_neighborhoods := Field{empty_text: "Областей на метод · 2..16 при уточнении"}
            gcc_search_max_slices := Field{empty_text: "Лимит отсеков поиска · не выше config"}
            gcc_search_config := Label{width: Fill text: "Ограничения config не загружены"}
            Caption{text: "Сухая статическая 2D-модель без порового давления и внешних нагрузок. Требуется инженерное рассмотрение."}
        }
    }
    mod.widgets.GeotechCircularResults = SolidView{
        width: 360 height: Fill flow: Down padding: 12 spacing: 6 draw_bg.color: #x17212b
        H3{width: Fill text: "Результаты / отсеки"}
        gcc_status := Label{width: Fill text: "NOT_RUN · постановка не загружена" draw_text.color: #xf2c47c}
        ScrollYView{width: Fill height: Fill flow: Down spacing: 6
            gcc_archive_status := Label{width: Fill text: "NO_RESULT · сохранённый расчёт не загружен" draw_text.color: #xf2c47c}
            gcc_archive_provenance := TextInput{width: Fill height: 180 is_multiline: true is_read_only: true text: "Нет сохранённого расчёта"}
            gcc_summary := TextInput{width: Fill height: 230 is_multiline: true is_read_only: true text: "Нет текущего результата"}
            Caption{text: "Сходимость ≠ физическая допустимость ≠ независимая верификация"}
            Caption{text: "Все силы — на 1 м ширины. Стрелки схематические."}
            View{width: Fill height: 32 flow: Right spacing: 5
                gcc_slices_prev := Button{width: Fill text: "← Отсеки"}
                gcc_slices_next := Button{width: Fill text: "Отсеки →"}
            }
            gcc_slices_page := Label{width: Fill text: "0 / 0"}
            gcc_slices_table := TextInput{width: Fill height: 430 is_multiline: true is_read_only: true text: "Нет отсеков"}
            gcc_slice_id := Field{empty_text: "ID отсека для выделения"}
            gcc_slice_select := Button{width: Fill text: "Выделить отсек"}
            Hr{}
            gcc_search_status := Label{width: Fill text: "Поиск NOT_RUN · numerical gate не принят"}
            gcc_search_minima := TextInput{width: Fill height: 110 is_multiline: true is_read_only: true text: "SEARCH_DOMAIN_LIMITED · minima NOT_PROVIDED"}
            gcc_candidate_selector := DropDown{width: Fill labels: ["Нет кандидатов"]}
            gcc_candidate_select := Button{width: Fill text: "Просмотреть кандидата"}
            gcc_candidate_apply := Button{width: Fill text: "Применить окружность кандидата"}
            gcc_candidates_table := TextInput{width: Fill height: 200 is_multiline: true is_read_only: true text: "Нет кандидатов"}
            gcc_report := Button{width: Fill text: "Получить текущий инженерный отчёт"}
            gcc_snapshot_save := Button{width: Fill text: "Сохранить расчёт в документе"}
            gcc_report_text := TextInput{width: Fill height: 280 is_multiline: true is_read_only: true text: "Нет текущего отчёта"}
            Caption{text: "3D EXTRUSION OF 2D MODEL\nNOT SPATIAL EQUILIBRIUM"}
        }
    }
    mod.widgets.GeotechCircularActions = SolidView{
        width: Fill height: Fit flow: Down padding: 6 spacing: 4 draw_bg.color: #x1d2b39
        ScrollXView{width: Fill height: 32 flow: Right spacing: 6
            gcc_ground_save := Button{text: "Сохранить разрез"}
            gcc_case_save := Button{text: "Сохранить case"}
            gcc_analyze := Button{text: "Analyze · Fellenius + Bishop" enabled: false}
            gcc_search := Button{text: "Критический поиск" enabled: false}
            gcc_cancel := Button{text: "Отмена" enabled: false}
            gcc_2d := Button{text: "2D"}
            gcc_3d := Button{text: "3D"}
            gcc_fit := Button{text: "Fit Geotech Model"}
        }
        ScrollXView{width: Fill height: 30 flow: Right spacing: 7
            gcc_ground := CheckBox{text: "Ground"}
            gcc_layers := CheckBox{text: "Soil Layers"}
            gcc_mass := CheckBox{text: "Failure Mass"}
            gcc_slices := CheckBox{text: "Slices"}
            gcc_forces := CheckBox{text: "Forces · Fellenius"}
            gcc_circle := CheckBox{text: "Selected Circle"}
        }
    }
}

const PAGE_SIZE: usize = 16;
const MAX_TEXT: usize = 4096;
const MAX_ID: usize = 256;
const MAX_CANDIDATE_ROWS: usize = 1000;
pub const CASE_EDITOR_RECORD_BYTES: usize = 256 << 10;

/// Bounded editor data for a circular case. The Document remains the owner of
/// any archive payload attached to the authoritative record.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CircularCaseEditorProjection {
    pub analysis_id: AnalysisCaseId,
    pub name: String,
    pub definition: CircularCaseDefinition,
    pub source_operation_id: OperationId,
}

/// Borrowed case fields suitable for catalog inspection before any field clone.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CircularCaseEditorView<'a> {
    pub analysis_id: AnalysisCaseId,
    pub name: &'a str,
    pub definition: &'a CircularCaseDefinition,
    pub source_operation_id: OperationId,
}

impl CircularCaseEditorProjection {
    pub fn view<'a>(case: &'a CircularAnalysisCase) -> CircularCaseEditorView<'a> {
        CircularCaseEditorView {
            analysis_id: case.analysis_id,
            name: &case.name,
            definition: &case.definition,
            source_operation_id: case.source_operation_id,
        }
    }

    /// Audit the borrowed editor fields before copying them out of the Document.
    pub fn from_case(case: &CircularAnalysisCase) -> Result<Self, String> {
        Self::view(case).serialized_bytes()?;
        let projection = Self {
            analysis_id: case.analysis_id,
            name: case.name.clone(),
            definition: case.definition.clone(),
            source_operation_id: case.source_operation_id,
        };
        validate_case_record(&projection)?;
        Ok(projection)
    }

    /// Match only the editor/source fields; archive state remains opaque here.
    pub fn matches_case(&self, case: &CircularAnalysisCase) -> bool {
        self.analysis_id == case.analysis_id
            && self.name == case.name
            && self.definition == case.definition
            && self.source_operation_id == case.source_operation_id
    }
}

impl CircularCaseEditorView<'_> {
    /// Exact compact JSON byte count, streamed without a JSON buffer.
    pub fn serialized_bytes(&self) -> Result<usize, String> {
        if self.name.trim().is_empty() || self.name.len() > MAX_TEXT {
            return Err(format!("CASE_EDITOR_NAME_BYTES · expected 1..={MAX_TEXT}"));
        }
        let slip_id = &self.definition.slip_surface.slip_id.0;
        if slip_id.trim().is_empty() || slip_id.len() > MAX_ID {
            return Err(format!("CASE_EDITOR_SLIP_ID_BYTES · expected 1..={MAX_ID}"));
        }

        let mut definition_counter = JsonByteCounter::new(CASE_EDITOR_RECORD_BYTES);
        if serde_json::to_writer(&mut definition_counter, self.definition).is_err() {
            return Err(if definition_counter.exceeded {
                format!("CASE_EDITOR_DEFINITION_BYTES_EXCEEDED · limit={CASE_EDITOR_RECORD_BYTES}")
            } else {
                "CASE_EDITOR_DEFINITION_SERIALIZATION_FAILED".into()
            });
        }

        let mut projection_counter = JsonByteCounter::new(CASE_EDITOR_RECORD_BYTES);
        if serde_json::to_writer(&mut projection_counter, self).is_err() {
            return Err(if projection_counter.exceeded {
                format!("CASE_EDITOR_RECORD_BYTES_EXCEEDED · limit={CASE_EDITOR_RECORD_BYTES}")
            } else {
                "CASE_EDITOR_RECORD_SERIALIZATION_FAILED".into()
            });
        }
        Ok(projection_counter.bytes)
    }
}

struct JsonByteCounter {
    bytes: usize,
    limit: usize,
    exceeded: bool,
}

impl JsonByteCounter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: 0,
            limit,
            exceeded: false,
        }
    }
}

impl std::io::Write for JsonByteCounter {
    fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        let Some(total) = self.bytes.checked_add(chunk.len()) else {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "serialized byte count overflow",
            ));
        };
        if total > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "serialized byte budget exceeded",
            ));
        }
        self.bytes = total;
        Ok(chunk.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Aliases are projections of these real controls into the existing UIA bridge,
/// not an independently published accessibility tree. Parent visibility applies.
pub(super) const CIRCULAR_INPUTS: &[(&str, LiveId)] = &[
    ("geotech.circular.ground.name", id!(gcc_ground_name)),
    ("geotech.circular.case.name", id!(gcc_case_name)),
    (
        "geotech.circular.view.extrusion_width",
        id!(gcc_preview_width),
    ),
    ("geotech.circular.circle.id", id!(gcc_slip_id)),
    ("geotech.circular.point.index", id!(gcc_point_index)),
    ("geotech.circular.point.x", id!(gcc_point_x)),
    ("geotech.circular.point.z", id!(gcc_point_z)),
    ("geotech.circular.interface.id", id!(gcc_interface_id)),
    ("geotech.circular.material.id", id!(gcc_material_id)),
    ("geotech.circular.material.name", id!(gcc_material_name)),
    ("geotech.circular.material.gamma", id!(gcc_gamma)),
    ("geotech.circular.material.cohesion", id!(gcc_cohesion)),
    ("geotech.circular.material.phi", id!(gcc_phi)),
    ("geotech.circular.material.provenance", id!(gcc_provenance)),
    ("geotech.circular.region.index", id!(gcc_region_index)),
    ("geotech.circular.region.id", id!(gcc_region_id)),
    ("geotech.circular.region.material", id!(gcc_region_material)),
    ("geotech.circular.circle.center_x", id!(gcc_center_x)),
    ("geotech.circular.circle.center_z", id!(gcc_center_z)),
    ("geotech.circular.circle.radius", id!(gcc_radius)),
    ("geotech.circular.circle.p1x", id!(gcc_p1x)),
    ("geotech.circular.circle.p1z", id!(gcc_p1z)),
    ("geotech.circular.circle.p2x", id!(gcc_p2x)),
    ("geotech.circular.circle.p2z", id!(gcc_p2z)),
    ("geotech.circular.circle.p3x", id!(gcc_p3x)),
    ("geotech.circular.circle.p3z", id!(gcc_p3z)),
    ("geotech.circular.slices.count", id!(gcc_slice_count)),
    ("geotech.circular.view.width", id!(gcc_width)),
    ("geotech.circular.search.xmin", id!(gcc_xmin)),
    ("geotech.circular.search.entry_xmin", id!(gcc_entry_xmin)),
    ("geotech.circular.search.entry_xmax", id!(gcc_entry_xmax)),
    ("geotech.circular.search.exit_xmin", id!(gcc_exit_xmin)),
    ("geotech.circular.search.exit_xmax", id!(gcc_exit_xmax)),
    (
        "geotech.circular.search.refinement_levels",
        id!(gcc_search_levels),
    ),
    (
        "geotech.circular.search.promising_neighborhoods",
        id!(gcc_search_neighborhoods),
    ),
    (
        "geotech.circular.search.max_slices",
        id!(gcc_search_max_slices),
    ),
    ("geotech.circular.search.xmax", id!(gcc_xmax)),
    ("geotech.circular.search.x_intervals", id!(gcc_xintervals)),
    ("geotech.circular.search.zmin", id!(gcc_zmin)),
    ("geotech.circular.search.zmax", id!(gcc_zmax)),
    ("geotech.circular.search.z_intervals", id!(gcc_zintervals)),
    ("geotech.circular.search.rmin", id!(gcc_rmin)),
    ("geotech.circular.search.rmax", id!(gcc_rmax)),
    ("geotech.circular.search.r_intervals", id!(gcc_rintervals)),
    (
        "geotech.circular.search.max_candidates",
        id!(gcc_candidates_limit),
    ),
    ("geotech.circular.search.max_ms", id!(gcc_time_limit)),
    ("geotech.circular.slice.id", id!(gcc_slice_id)),
];
pub(super) const CIRCULAR_BUTTONS: &[(&str, LiveId)] = &[
    ("geotech.circular.ground.new", id!(gcc_new)),
    ("geotech.circular.ground.reload", id!(gcc_reload)),
    ("geotech.circular.ground.save", id!(gcc_ground_save)),
    ("geotech.circular.case.new", id!(gcc_case_new)),
    ("geotech.circular.case.reload", id!(gcc_case_reload)),
    ("geotech.circular.case.save", id!(gcc_case_save)),
    ("geotech.circular.case.config.edit", id!(gcc_config_edit)),
    (
        "geotech.circular.search.policy.edit",
        id!(gcc_search_policy),
    ),
    ("geotech.circular.search.save", id!(gcc_search_save)),
    ("geotech.circular.search.freeze", id!(gcc_search_freeze)),
    ("geotech.circular.analyze", id!(gcc_analyze)),
    ("geotech.circular.search", id!(gcc_search)),
    ("geotech.circular.cancel", id!(gcc_cancel)),
    ("geotech.circular.point.load", id!(gcc_point_load)),
    ("geotech.circular.point.add", id!(gcc_point_add)),
    ("geotech.circular.point.apply", id!(gcc_point_apply)),
    ("geotech.circular.point.remove", id!(gcc_point_remove)),
    ("geotech.circular.material.apply", id!(gcc_material_apply)),
    ("geotech.circular.material.remove", id!(gcc_material_remove)),
    ("geotech.circular.interface.add", id!(gcc_interface_add)),
    (
        "geotech.circular.interface.remove",
        id!(gcc_interface_remove),
    ),
    ("geotech.circular.region.load", id!(gcc_region_load)),
    ("geotech.circular.region.apply", id!(gcc_region_apply)),
    ("geotech.circular.region.remove", id!(gcc_region_remove)),
    ("geotech.circular.circle.preview", id!(gcc_circle_preview)),
    ("geotech.circular.view.2d", id!(gcc_2d)),
    ("geotech.circular.view.3d", id!(gcc_3d)),
    ("geotech.circular.view.fit", id!(gcc_fit)),
    ("geotech.circular.slices.prev", id!(gcc_slices_prev)),
    ("geotech.circular.slices.next", id!(gcc_slices_next)),
    ("geotech.circular.slice.select", id!(gcc_slice_select)),
    (
        "geotech.circular.candidate.select",
        id!(gcc_candidate_select),
    ),
    ("geotech.circular.candidate.apply", id!(gcc_candidate_apply)),
    ("geotech.circular.report", id!(gcc_report)),
    ("geotech.circular.snapshot.save", id!(gcc_snapshot_save)),
];
pub(super) const CIRCULAR_SELECTORS: &[(&str, LiveId)] = &[
    ("geotech.circular.ground.select", id!(gcc_ground_selector)),
    ("geotech.circular.case.select", id!(gcc_case_selector)),
    (
        "geotech.circular.case.ground_reference",
        id!(gcc_case_ground),
    ),
    (
        "geotech.circular.search.target_method",
        id!(gcc_search_target),
    ),
    ("geotech.circular.point.boundary", id!(gcc_point_boundary)),
    (
        "geotech.circular.interface.select",
        id!(gcc_interface_selector),
    ),
    (
        "geotech.circular.material.select",
        id!(gcc_material_selector),
    ),
    ("geotech.circular.circle.mode", id!(gcc_circle_mode)),
    ("geotech.circular.circle.direction", id!(gcc_direction)),
    (
        "geotech.circular.candidate.row",
        id!(gcc_candidate_selector),
    ),
];
pub(super) const CIRCULAR_TOGGLES: &[(&str, LiveId)] = &[
    ("geotech.circular.search.enabled", id!(gcc_search_enabled)),
    ("geotech.circular.slices.refine", id!(gcc_refine)),
    ("geotech.circular.layer.ground", id!(gcc_ground)),
    ("geotech.circular.layer.soil_layers", id!(gcc_layers)),
    ("geotech.circular.layer.failure_mass", id!(gcc_mass)),
    ("geotech.circular.layer.slices", id!(gcc_slices)),
    ("geotech.circular.layer.forces", id!(gcc_forces)),
    ("geotech.circular.layer.circle", id!(gcc_circle)),
];
pub(super) const CIRCULAR_OUTPUTS: &[(&str, LiveId)] = &[
    ("geotech.circular.ground.id", id!(gcc_ground_id)),
    ("geotech.circular.analysis.id", id!(gcc_analysis_id)),
    ("geotech.circular.ground.readiness", id!(gcc_readiness)),
    ("geotech.circular.case.source_stamps", id!(gcc_case_sources)),
    ("geotech.circular.case.full_config", id!(gcc_full_config)),
    ("geotech.circular.search.minima", id!(gcc_search_minima)),
    ("geotech.circular.search.config", id!(gcc_search_config)),
    ("geotech.circular.binding", id!(gcc_binding)),
    ("geotech.circular.status", id!(gcc_status)),
    ("geotech.circular.results", id!(gcc_summary)),
    ("geotech.circular.geometry.rows", id!(gcc_geometry_rows)),
    ("geotech.circular.region.rows", id!(gcc_region_rows)),
    ("geotech.circular.slices.page", id!(gcc_slices_page)),
    ("geotech.circular.slices.table", id!(gcc_slices_table)),
    ("geotech.circular.archive.status", id!(gcc_archive_status)),
    ("geotech.circular.archive.provenance", id!(gcc_archive_provenance)),
    ("geotech.circular.search.status", id!(gcc_search_status)),
    (
        "geotech.circular.candidates.table",
        id!(gcc_candidates_table),
    ),
    ("geotech.circular.report.text", id!(gcc_report_text)),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DocumentBinding {
    pub document_uuid: String,
    pub session_id: String,
    pub open_generation: u64,
    /// Transaction precondition/provenance, not a mathematical source stamp.
    pub revision: u64,
}
impl DocumentBinding {
    fn same_runtime(&self, other: &Self) -> bool {
        !self.document_uuid.is_empty()
            && !self.session_id.is_empty()
            && self.document_uuid == other.document_uuid
            && self.session_id == other.session_id
            && self.open_generation == other.open_generation
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CalculationIdentity {
    pub document: DocumentBinding,
    pub ground_id: DocumentGroundId,
    pub analysis_id: AnalysisCaseId,
    pub ground_source_operation_id: OperationId,
    pub case_source_operation_id: OperationId,
    pub config: CircularAnalysisConfig,
    pub source_revision: String,
    pub input_fingerprint: String,
    pub source_fingerprint: String,
    pub solver_version: String,
}
impl CalculationIdentity {
    fn same_source(&self, other: &Self) -> bool {
        self.document.same_runtime(&other.document)
            && self.ground_id == other.ground_id
            && self.analysis_id == other.analysis_id
            && self.ground_source_operation_id == other.ground_source_operation_id
            && self.case_source_operation_id == other.case_source_operation_id
            && self.config == other.config
            && self.source_revision == other.source_revision
            && self.input_fingerprint == other.input_fingerprint
            && self.source_fingerprint == other.source_fingerprint
            && self.solver_version == other.solver_version
    }
    fn matches(&self, result: &CircularSlopeResult) -> bool {
        !self.input_fingerprint.is_empty()
            && !self.source_fingerprint.is_empty()
            && !self.solver_version.is_empty()
            && self.analysis_id.to_string() == result.analysis_id
            && self.source_revision == result.source_revision
            && self.input_fingerprint == result.input_fingerprint
            && self.source_fingerprint == result.source_fingerprint
            && self.solver_version == result.solver_version
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PhysicalStatus {
    NotAssessed,
    Admissible,
    Inadmissible,
}
#[derive(Clone, Debug)]
pub(super) struct MethodReview {
    pub physical_status: PhysicalStatus,
    pub explanation: String,
    pub independently_verified: bool,
}
impl Default for MethodReview {
    fn default() -> Self {
        Self {
            physical_status: PhysicalStatus::NotAssessed,
            explanation: "Физическая допустимость не подтверждена ядром".into(),
            independently_verified: false,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub(super) struct ResultReview {
    pub fellenius: MethodReview,
    pub bishop: MethodReview,
    /// Actual finite-table contact assessment; never a whole-arc proof.
    pub finite_table: MethodReview,
    /// Separate from finite-slice contact: the host maps the final core's
    /// geometry-aware endpoint guard. Missing/unknown is NotAssessed.
    pub geometry_endpoints: MethodReview,
    /// Host-supplied actual core status. A root or endpoint screen cannot set it.
    pub whole_arc: MethodReview,
    /// Required declared model/domain for any admissible whole-arc claim.
    pub whole_arc_applicability: Option<String>,
}
impl ResultReview {
    fn whole_arc_status(&self) -> PhysicalStatus {
        if self.whole_arc.physical_status == PhysicalStatus::Admissible
            && self
                .whole_arc_applicability
                .as_ref()
                .is_none_or(|s| s.trim().is_empty())
        {
            PhysicalStatus::NotAssessed
        } else {
            self.whole_arc.physical_status
        }
    }
    fn whole_arc_admissible(&self) -> bool {
        self.whole_arc_status() == PhysicalStatus::Admissible
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JobKind {
    Analyze,
    Search,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct JobTicket {
    pub sequence: u64,
    pub search_generation: u64,
    pub kind: JobKind,
    pub identity: CalculationIdentity,
    /// Full frozen search spec; None only for specified-circle mode.
    pub search_spec: Option<SearchBox>,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SearchBox {
    pub request: SearchPublicRequestV1,
    pub analysis_config: CircularAnalysisConfig,
}
/// A load is an explicit editor replacement, fenced against delayed callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordLoadMode {
    Ground,
    Case,
    NewCase,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EditorContext {
    pub load_generation: u64,
    pub resolved_identity: Option<CalculationIdentity>,
    pub document: DocumentBinding,
    pub draft_generation: u64,
    pub ground_generation: u64,
    pub case_generation: u64,
    pub search_generation: u64,
    pub ground_source: Option<(DocumentGroundId, OperationId)>,
    pub case_source: Option<(AnalysisCaseId, OperationId)>,
    pub loaded_config: Option<CircularAnalysisConfig>,
    pub frozen_search: Option<SearchBox>,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RecordLoadTicket {
    pub context: EditorContext,
    pub mode: RecordLoadMode,
    pub ground: (DocumentGroundId, OperationId),
    pub case: Option<(AnalysisCaseId, OperationId)>,
    pub case_config: Option<CircularAnalysisConfig>,
}
#[derive(Clone, Debug)]
pub(super) struct SaveGroundProposal {
    pub context: EditorContext,
    pub expected: DocumentBinding,
    pub generation: u64,
    pub expected_ground: Option<(DocumentGroundId, OperationId)>,
    pub before: Option<Arc<GroundSection>>,
    pub command: EngineeringCommand,
}
#[derive(Clone, Debug)]
pub(super) struct SaveCaseProposal {
    pub context: EditorContext,
    pub expected: DocumentBinding,
    pub generation: u64,
    pub expected_case: Option<(AnalysisCaseId, OperationId)>,
    pub ground_generation: u64,
    pub before: Option<Arc<CircularCaseEditorProjection>>,
    pub referenced_ground: Arc<GroundSection>,
    pub expected_ground: (DocumentGroundId, OperationId),
    pub command: EngineeringCommand,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ReportTicket {
    pub identity: CalculationIdentity,
    pub search_generation: u64,
    pub candidate_source: Option<JobTicket>,
}
impl ReportTicket {
    pub fn same_source(&self, other: &Self) -> bool {
        self.identity.same_source(&other.identity)
            && self.search_generation == other.search_generation
            && self.candidate_source == other.candidate_source
    }
}
pub(super) enum BoundaryTarget {
    Terrain,
    Bottom,
    Interface(String),
}
pub(super) enum CircularUiIntent {
    LoadGround(RecordLoadTicket),
    LoadCase(RecordLoadTicket),
    /// Host supplies the explicit full config/basis/units, not a fake circle.
    NewCase(RecordLoadTicket),
    EditFullConfig {
        context: EditorContext,
        config: CircularAnalysisConfig,
    },
    /// Opens the host-owned policy editor with the current complete typed draft,
    /// or None when no explicit policy template has been supplied yet.
    EditSearchPolicy {
        context: EditorContext,
        request: Option<SearchPublicRequestV1>,
    },
    SaveGround(SaveGroundProposal),
    SaveCase(SaveCaseProposal),
    Analyze(JobTicket),
    Search(JobTicket, SearchBox),
    Cancel(JobTicket),
    PreviewCircle(CircularSlipSurface),
    SelectSlice(usize),
    SelectCandidate {
        search: JobTicket,
        id: CandidateId,
    },
    ApplyCandidate {
        expected: DocumentBinding,
        search: JobTicket,
        id: CandidateId,
    },
    ViewChanged,
    Fit,
    Report(ReportTicket),
    SaveSnapshot(ReportTicket),
}

/// An archive is a read-only view of saved evidence, never a current job ticket.
/// The worker owns integrity/currentness; this fence owns local editor lifetime.
struct NativeArchiveView {
    context: EditorContext,
    inspection: Arc<ArchiveInspection>,
    review: ResultReview,
}

/// Local draft only. Host owns all authoritative changes and shared history.
pub(super) struct CircularPanel {
    pub active: bool,
    pub flags: OverlayFlags,
    pub view_3d: bool,
    pub dirty: bool,
    pub stale: bool,
    pub draft_generation: u64,
    binding: Option<DocumentBinding>,
    live_binding: Option<DocumentBinding>,
    saved_input: Option<Arc<CircularAnalysisInput>>,
    draft_ground: Option<GroundDefinition>,
    saved_ground: Option<Arc<GroundSection>>,
    saved_case: Option<Arc<CircularCaseEditorProjection>>,
    catalog_binding: Option<DocumentBinding>,
    grounds: Vec<Arc<GroundSection>>,
    cases: Vec<Arc<CircularCaseEditorProjection>>,
    ground_pick: Option<DocumentGroundId>,
    case_pick: Option<AnalysisCaseId>,
    case_ground_ref: Option<DocumentGroundId>,
    ground_generation: u64,
    case_generation: u64,
    ground_dirty: bool,
    case_dirty: bool,
    search_dirty: bool,
    case_config_loaded: bool,
    basis: AnalysisBasis,
    units: InputUnits,
    circle_preview: Option<CircularSlipSurface>,
    selected_slice: Option<usize>,
    selected_candidate: Option<CandidateId>,
    preview_width: Option<f64>,
    preview_generation: u64,
    load_generation: u64,
    analysis_gate_accepted: bool,
    config: CircularAnalysisConfig,
    saved_search_domain: Option<SearchBox>,
    search_request_template: Option<SearchPublicRequestV1>,
    resolved_search: Option<ResolvedCircularSearch>,
    identity: Option<CalculationIdentity>,
    result: Option<Arc<CircularSlopeResult>>,
    result_source: Option<JobTicket>,
    review: ResultReview,
    archive: Option<NativeArchiveView>,
    archive_error: Option<String>,
    pending_rows: BTreeSet<&'static str>,
    job: Option<JobTicket>,
    cancelling: bool,
    sequence: u64,
    search_generation: u64,
    material_selection: usize,
    point_boundary: usize,
    interface_selection: usize,
    search_gate_accepted: bool,
    candidate_source: Option<JobTicket>,
    search_result: Option<Arc<SearchResultV1>>,
    page: usize,
    status: String,
}
impl Default for CircularPanel {
    fn default() -> Self {
        Self {
            active: false,
            flags: OverlayFlags::default(),
            view_3d: false,
            dirty: false,
            stale: false,
            draft_generation: 0,
            binding: None,
            live_binding: None,
            saved_input: None,
            draft_ground: None,
            saved_ground: None,
            saved_case: None,
            catalog_binding: None,
            grounds: Vec::new(),
            cases: Vec::new(),
            ground_pick: None,
            case_pick: None,
            case_ground_ref: None,
            ground_generation: 0,
            case_generation: 0,
            ground_dirty: false,
            case_dirty: false,
            search_dirty: false,
            case_config_loaded: false,
            basis: AnalysisBasis::DryStatic,
            units: InputUnits::default(),
            circle_preview: None,
            selected_slice: None,
            selected_candidate: None,
            preview_width: None,
            preview_generation: 0,
            load_generation: 0,
            analysis_gate_accepted: false,
            config: CircularAnalysisConfig::default(),
            saved_search_domain: None,
            search_request_template: None,
            resolved_search: None,
            identity: None,
            result: None,
            result_source: None,
            review: ResultReview::default(),
            archive: None,
            archive_error: None,
            pending_rows: BTreeSet::new(),
            job: None,
            cancelling: false,
            sequence: 0,
            search_generation: 0,
            material_selection: 0,
            point_boundary: 0,
            interface_selection: 0,
            search_gate_accepted: false,
            candidate_source: None,
            search_result: None,
            page: 0,
            status: "NOT_RUN".into(),
        }
    }
}
impl CircularPanel {
    /// Owner decisions only; source presence/runtime accepted is never a gate grant.
    pub fn set_host_gates(
        &mut self,
        expected: &EditorContext,
        analyze: bool,
        search: bool,
    ) -> Result<(), String> {
        if self.editor_context()? != *expected {
            return Err("STALE_HOST_GATE · решение другого snapshot не применяется".into());
        }
        let ready = self.ready_for_analysis();
        self.analysis_gate_accepted = analyze && ready;
        self.search_gate_accepted = search && ready && self.search_dispatch_ready();
        Ok(())
    }
    /// Attach an owner context; a changed revision needs actual record observation.
    pub fn attach_document(&mut self, live: DocumentBinding) {
        if self
            .live_binding
            .as_ref()
            .is_some_and(|old| !old.same_runtime(&live))
        {
            self.clear_local_interaction_state();
            self.clear_archive_view();
            self.grounds.clear();
            self.cases.clear();
            self.catalog_binding = None;
            self.ground_pick = None;
            self.case_pick = None;
        }
        let first = self.binding.is_none();
        let unchanged = self.binding.as_ref() == Some(&live);
        if first && require_runtime(&live).is_ok() {
            self.binding = Some(live.clone());
        } else if !unchanged {
            self.clear_local_interaction_state();
            self.stale = true;
            self.analysis_gate_accepted = false;
            self.search_gate_accepted = false;
            self.status = "STALE · нужен актуальный Ground/case snapshot; текст сохранён".into();
        }
        self.live_binding = Some(live);
    }
    pub fn set_catalog(
        &mut self,
        live: DocumentBinding,
        grounds: Vec<Arc<GroundSection>>,
        cases: Vec<Arc<CircularCaseEditorProjection>>,
    ) -> Result<(), String> {
        require_runtime(&live)?;
        if self
            .live_binding
            .as_ref()
            .is_some_and(|old| !old.same_runtime(&live) || live.revision < old.revision)
        {
            return Err("STALE_CATALOG_CONTEXT · сначала attach текущего Document".into());
        }
        if grounds.len() > 256 || cases.len() > 1024 {
            return Err("Превышен явный лимит списка: 256 разрезов / 1024 cases".into());
        }
        let ids: BTreeSet<_> = grounds.iter().map(|g| g.ground_section_id).collect();
        let case_ids: BTreeSet<_> = cases.iter().map(|c| c.analysis_id).collect();
        if ids.len() != grounds.len()
            || case_ids.len() != cases.len()
            || grounds.iter().any(|g| {
                g.ground_section_id.get() == 0
                    || g.source_operation_id.get() < g.ground_section_id.get()
            })
            || cases.iter().any(|c| {
                c.analysis_id.get() == 0
                    || c.source_operation_id.get() < c.analysis_id.get()
                    || !ids.contains(&c.definition.ground_section_id)
            })
        {
            return Err("Неоднозначный или неполный каталог Ground/case".into());
        }
        let ground = self
            .saved_ground
            .as_ref()
            .and_then(|old| {
                grounds
                    .iter()
                    .find(|g| g.ground_section_id == old.ground_section_id)
            })
            .cloned();
        let case = self
            .saved_case
            .as_ref()
            .and_then(|old| cases.iter().find(|c| c.analysis_id == old.analysis_id))
            .cloned();
        self.observe_document(live.clone(), ground.as_deref(), case.as_deref());
        self.catalog_binding = Some(live);
        self.grounds = grounds;
        self.cases = cases;
        Ok(())
    }
    /// Pass current selected records from the same authoritative Document, including
    /// selected records outside a visible page. None means absent, not "not fetched".
    pub fn observe_document(
        &mut self,
        live: DocumentBinding,
        ground: Option<&GroundSection>,
        case: Option<&CircularCaseEditorProjection>,
    ) {
        if self
            .live_binding
            .as_ref()
            .is_some_and(|old| !old.same_runtime(&live) || live.revision < old.revision)
        {
            return;
        }
        let same = self
            .binding
            .as_ref()
            .is_none_or(|old| old.same_runtime(&live) && old.revision <= live.revision);
        let ground_same = self.saved_ground.as_ref().is_none_or(|old| {
            ground.is_some_and(|g| {
                g.ground_section_id == old.ground_section_id
                    && g.source_operation_id == old.source_operation_id
                    && g.definition == old.definition
            })
        });
        let case_same = self.saved_case.as_ref().is_none_or(|old| {
            case.is_some_and(|c| {
                c.analysis_id == old.analysis_id
                    && c.source_operation_id == old.source_operation_id
                    && c.definition == old.definition
            })
        });
        if !same {
            self.grounds.clear();
            self.cases.clear();
            self.catalog_binding = None;
            self.ground_pick = None;
            self.case_pick = None;
        }
        self.live_binding = Some(live.clone());
        if !same || !ground_same || !case_same {
            self.clear_local_interaction_state();
            self.clear_archive_view();
            self.stale = true;
            self.analysis_gate_accepted = false;
            self.search_gate_accepted = false;
            self.status="STALE · изменён ground/case source или сеанс; текст сохранён, требуется явная загрузка".into();
            return;
        }
        // A rename, no-op or unrelated project revision cannot change numerical identity.
        self.binding = Some(live.clone());
        self.stale = false;
        if let Some(g) = ground {
            if self.saved_ground.is_some() {
                self.saved_ground = Some(Arc::new(g.clone()));
            }
        }
        if let Some(c) = case {
            if self.saved_case.is_some() {
                self.saved_case = Some(Arc::new(c.clone()));
            }
        }
        if let Some(identity) = &mut self.identity {
            identity.document = live;
        }
    }
    fn bound(&self) -> bool {
        !self.stale
            && self
                .binding
                .as_ref()
                .zip(self.live_binding.as_ref())
                .is_some_and(|(a, b)| a.same_runtime(b))
    }
    fn update_dirty(&mut self) {
        self.dirty = self.ground_dirty || self.case_dirty || self.search_dirty;
    }
    fn changed(&mut self) {
        self.clear_local_interaction_state();
        self.archive_error = None;
        self.case_dirty = true;
        self.case_generation = self.case_generation.saturating_add(1);
        self.draft_generation = self.draft_generation.saturating_add(1);
        self.update_dirty();
        self.page = 0;
        self.status = "DIRTY CASE · сохраните case; прежние результаты скрыты".into();
    }
    fn ground_changed(&mut self) {
        self.clear_local_interaction_state();
        self.archive_error = None;
        self.ground_dirty = true;
        self.ground_generation = self.ground_generation.saturating_add(1);
        self.draft_generation = self.draft_generation.saturating_add(1);
        self.update_dirty();
        self.page = 0;
        self.status = "DIRTY GROUND · сохраните разрез; связанные результаты скрыты".into();
    }
    fn invalidate_source(&mut self) {
        self.clear_local_interaction_state();
        self.clear_archive_view();
        self.identity = None;
        self.saved_input = None;
        self.result = None;
        self.result_source = None;
        self.review = ResultReview::default();
        self.search_result = None;
        self.candidate_source = None;
        self.saved_search_domain = None;
        self.search_request_template = None;
        self.resolved_search = None;
        self.analysis_gate_accepted = false;
        self.search_gate_accepted = false;
        self.search_generation = self.search_generation.saturating_add(1);
    }
    fn clear_local_interaction_state(&mut self) {
        self.circle_preview = None;
        self.selected_slice = None;
        self.selected_candidate = None;
    }
    fn clear_archive_view(&mut self) {
        self.archive = None;
        self.archive_error = None;
        self.page = 0;
    }
    pub fn invalidate_archive_view(&mut self) {
        self.clear_archive_view();
    }
    fn archive_context_matches(&self, expected: &EditorContext) -> bool {
        self.bound()
            && !self.dirty
            && self.live_binding.as_ref().is_some_and(|live| {
                live.same_runtime(&expected.document)
            })
            && expected.load_generation == self.load_generation
            && expected.draft_generation == self.draft_generation
            && expected.ground_generation == self.ground_generation
            && expected.case_generation == self.case_generation
            && expected.ground_source == self.saved_ground.as_ref()
                .map(|g| (g.ground_section_id, g.source_operation_id))
            && expected.case_source == self.saved_case.as_ref()
                .map(|c| (c.analysis_id, c.source_operation_id))
    }
    pub fn archive_load_context(&self) -> Option<EditorContext> {
        (!self.dirty && self.bound() && self.saved_case.is_some())
            .then(|| self.editor_context().ok()).flatten()
    }
    pub fn archive_autoload_ticket(&mut self, id: AnalysisCaseId) -> Result<Option<RecordLoadTicket>, String> {
        if self.dirty || !self.pending_rows.is_empty() || self.job.is_some() {
            return Ok(None);
        }
        let ground = self.cases.iter().find(|case| case.analysis_id == id)
            .ok_or("ARCHIVE_AUTOLOAD_CASE_NOT_IN_CATALOG")?.definition.ground_section_id;
        self.record_load_ticket(RecordLoadMode::Case, ground, Some(id)).map(Some)
    }
    pub fn accept_archive(
        &mut self,
        expected: &EditorContext,
        inspection: Arc<ArchiveInspection>,
        review: ResultReview,
    ) -> Result<(), String> {
        if !self.archive_context_matches(expected) {
            return Err("STALE_ARCHIVE_VIEW · изменён Document, case или локальный черновик".into());
        }
        if Some(inspection.analysis_id) != expected.case_source.map(|value| value.0) {
            return Err("ARCHIVE_CASE_VIEW_MISMATCH".into());
        }
        if let Some(snapshot) = &inspection.snapshot {
            let saved = &snapshot.archive().bundle.context;
            if Some(saved.case_id.0) != expected.case_source.map(|v| v.0.get()) {
                return Err("ARCHIVE_CASE_VIEW_MISMATCH".into());
            }
        }
        self.archive_error = None;
        self.archive = Some(NativeArchiveView {
            context: expected.clone(), inspection, review,
        });
        self.page = 0;
        Ok(())
    }
    pub fn archive_load_failed(&mut self, expected: &EditorContext, error: &str) {
        if self.archive_context_matches(expected) {
            self.archive = None;
            self.archive_error = Some(bounded(error, 512));
        }
    }
    pub fn archived_view(&self) -> Option<&ArchiveInspection> {
        // Immutable history stays readable after a local edit. It cannot cross
        // an Open, explicit editor replacement or loaded case identity change.
        self.archive.as_ref().filter(|view| {
            self.live_binding.as_ref().is_some_and(|live| live.same_runtime(&view.context.document))
                && view.context.load_generation == self.load_generation
                && view.context.case_source.map(|v|v.0) == self.saved_case.as_ref().map(|c|c.analysis_id)
                && view.context.ground_source.map(|v|v.0) == self.saved_ground.as_ref().map(|g|g.ground_section_id)
        })
            .map(|view| view.inspection.as_ref())
    }
    fn archive_matches_editor(&self) -> bool {
        self.archive.as_ref().is_some_and(|view| self.archive_context_matches(&view.context))
    }
    pub fn archived_specified(&self) -> Option<(&CircularAnalysisInput, &CircularAnalysisConfig, &CircularSlopeResult)> {
        match &self.archived_view()?.snapshot.as_ref()?.archive().bundle.evidence {
            geotech_report::ReportEvidence::Specified { input, config, result } => Some((input, config, result)),
            _ => None,
        }
    }
    /// Textual diagnostic readback may show explicitly labelled stale evidence.
    /// It never grants new-job save/report/apply authority or current overlays.
    pub fn display_result(&self) -> Option<&CircularSlopeResult> {
        if self.current_search_result().is_some() { return None; }
        self.current_result().or_else(|| self.archived_specified().map(|v| v.2))
    }
    pub fn display_search_result(&self) -> Option<&SearchResultV1> {
        if self.current_result().is_some() { return None; }
        self.current_search_result().or_else(|| {
            match &self.archived_view()?.snapshot.as_ref()?.archive().bundle.evidence {
                geotech_report::ReportEvidence::Search { result, .. } => Some(result),
                _ => None,
            }
        })
    }
    pub fn presentation_state(&self) -> &'static str {
        if self.job.is_some() { return "CALCULATING"; }
        if self.current_result().is_some() || self.current_search_result().is_some() {
            return "CURRENT_CALCULATED";
        }
        if self.archive_error.is_some() || self.status.starts_with("FAILED") || self.status.starts_with("HOST_RESULT_DISCARDED") {
            return "FAILED";
        }
        match self.archived_view().map(|v| v.presentation.state) {
            Some(ArchiveState::ArchivedCurrent) if !self.archive_matches_editor() => "ARCHIVED_STALE",
            Some(ArchiveState::ArchivedCurrent) => "ARCHIVED_CURRENT",
            Some(ArchiveState::ArchivedStale) => "ARCHIVED_STALE",
            _ => "NO_RESULT",
        }
    }
    fn archive_banner(&self) -> String {
        if let Some(error) = &self.archive_error {
            return format!("ARCHIVE_INVALID / FAILED · {error}");
        }
        let history = if self.job.is_some() || self.current_result().is_some() || self.current_search_result().is_some() {
            "История · "
        } else { "" };
        match self.archived_view().map(|v| v.presentation.state) {
            Some(ArchiveState::ArchivedCurrent) if !self.archive_matches_editor() => format!("{history}ARCHIVED_STALE · архив актуален для сохранённых входов; локальный черновик изменён, показана история"),
            Some(ArchiveState::ArchivedCurrent) => format!("{history}ARCHIVED_CURRENT · архив актуален для сохранённых входов; это не новый расчёт"),
            Some(ArchiveState::ArchivedStale) => format!("{history}ARCHIVED_STALE · прежние значения; к текущим входам не применяются"),
            _ => "NO_RESULT · сохранённый расчёт отсутствует или ещё проверяется".into(),
        }
    }
    fn archive_provenance_text(&self) -> String {
        let Some(view) = self.archived_view() else {
            return self.archive_error.clone().unwrap_or_else(|| "Сохранённый расчёт не загружен".into());
        };
        let Some(snapshot) = &view.snapshot else { return "Для выбранного case расчёт не сохранён".into(); };
        let archive = snapshot.archive();
        let context = &archive.bundle.context;
        let mut text = DisplayText::new(32 << 10);
        let (method, revision, fingerprint) = match &archive.bundle.evidence {
            geotech_report::ReportEvidence::Specified { result, .. } => (
                "Fellenius Ordinary + Bishop Simplified · заданная окружность", result.source_revision.as_str(), result.input_fingerprint.as_str()),
            geotech_report::ReportEvidence::Search { request, result } => (
                "Fellenius + Bishop · поиск в исследованном семействе", request.source_revision.as_str(), result.request_fingerprint.as_str()),
        };
        let _ = writeln!(text, "Метод: {method}\nProject: {}\nCase: {} · Ground: {}\nСохранённая ревизия: {}\nGround/case source: {} / {}\nSource revision: {revision}\nInput/request fingerprint: {fingerprint}\nSolver: {}\nSolver source: {}\nArchive digest: {}\nReport digest: {}\nРасчёт в этом сеансе: не запускался для этого архива\nSaved worker arithmetic: {} · независимая проверка не предоставлена", context.project_id.0, context.case_id.0, context.ground_id.0, context.captured_global_revision, context.ground_source_operation.0, context.case_source_operation.0, context.archived_solver_version, context.archived_solver_source_fingerprint, archive.archive_digest, archive.report_digest, context.saved_calculation_worker_arithmetic.as_ref().map_or("UNKNOWN", |value| if value.accepted { "ACCEPTED" } else { "REJECTED" }));
        for source in &archive.bundle.source_references {
            if text.truncated { break; }
            let _ = writeln!(text, "Источник: {} · {:?}\n{}", source.title, source.availability, source.note.as_deref().unwrap_or(""));
        }
        text.finish()
    }
    fn context_matches_current(&self, expected: &EditorContext) -> bool {
        self.bound()
            && self.live_binding.as_ref() == Some(&expected.document)
            && expected.load_generation == self.load_generation
            && expected.resolved_identity == self.identity
            && expected.draft_generation == self.draft_generation
            && expected.ground_generation == self.ground_generation
            && expected.case_generation == self.case_generation
            && expected.search_generation == self.search_generation
            && expected.ground_source
                == self
                    .saved_ground
                    .as_ref()
                    .map(|g| (g.ground_section_id, g.source_operation_id))
            && expected.case_source
                == self
                    .saved_case
                    .as_ref()
                    .map(|c| (c.analysis_id, c.source_operation_id))
            && expected.loaded_config == self.case_config_loaded.then_some(self.config)
            && expected.frozen_search.as_ref() == self.saved_search_domain.as_ref()
    }
    fn ready_for_analysis(&self) -> bool {
        self.bound()
            && !self.ground_dirty
            && !self.case_dirty
            && self.case_config_loaded
            && self
                .saved_ground
                .as_ref()
                .zip(self.saved_case.as_ref())
                .is_some_and(|(g, c)| {
                    c.definition.ground_section_id == g.ground_section_id
                        && inspect_ground_readiness(&g.definition).is_ok_and(|r| r.is_ready())
                })
            && self.identity.is_some()
            && self.saved_input.is_some()
    }
    /// Search has its own persisted-policy and Document-resolution contract.
    /// This is deliberately separate from mathematical/analysis readiness.
    fn search_dispatch_ready(&self) -> bool {
        if !self.ready_for_analysis() || self.search_dirty {
            return false;
        }
        let (Some(ground), Some(case), Some(input), Some(resolved), Some(template), Some(frozen)) = (
            self.saved_ground.as_deref(),
            self.saved_case.as_deref(),
            self.saved_input.as_deref(),
            self.resolved_search.as_ref(),
            self.search_request_template.as_ref(),
            self.saved_search_domain.as_ref(),
        ) else {
            return false;
        };
        let Some(saved_spec) = case.definition.search_spec.as_ref() else {
            return false;
        };
        resolved.ground_source_operation_id == ground.source_operation_id
            && resolved.case_source_operation_id == case.source_operation_id
            && resolved.request == *template
            && resolved.request == frozen.request
            && frozen.analysis_config == self.config
            && search_matches_input(&resolved.request, input)
            && resolved.request.shared_analysis == search_analysis_config(&case.definition.config)
            && case.definition.config == self.config
            && circular_search_spec(&resolved.request) == *saved_spec
            && validate_search_box(frozen).is_ok()
    }
    pub fn current_result(&self) -> Option<&CircularSlopeResult> {
        if !self.ready_for_analysis() {
            return None;
        }
        self.result
            .as_deref()
            .filter(|r| self.identity.as_ref().is_some_and(|i| i.matches(r)))
    }
    /// Store an already validated display-only circle against the current saved
    /// Ground source. This never invokes analysis or changes the engineering draft.
    pub fn set_circle_preview(
        &mut self,
        expected: &EditorContext,
        circle: CircularSlipSurface,
    ) -> Result<(), String> {
        if !self.context_matches_current(expected)
            || self.ground_dirty
            || !self.case_config_loaded
            || !self.saved_ground.as_ref().is_some_and(|ground| {
                inspect_ground_readiness(&ground.definition).is_ok_and(|r| r.is_ready())
            })
        {
            return Err(
                "STALE_CIRCLE_PREVIEW · сохранённый Ground или editor snapshot изменился".into(),
            );
        }
        self.circle_preview = Some(circle);
        Ok(())
    }
    pub fn circle_preview(&self) -> Option<&CircularSlipSurface> {
        self.circle_preview.as_ref()
    }
    /// Select a row from the current numerical table. Currentness is not an
    /// acceptance or physical-review decision.
    pub fn select_slice(&mut self, expected: &EditorContext, id: usize) -> Result<(), String> {
        if !self.context_matches_current(expected) {
            return Err("STALE_SLICE_SELECTION · editor snapshot изменился".into());
        }
        let Some(index) = self
            .display_result()
            .and_then(|result| result.slices.iter().position(|slice| slice.slice_id == id))
        else {
            return Err("SLICE_NOT_CURRENT · такого ID нет в текущей таблице".into());
        };
        self.selected_slice = Some(id);
        self.page = index / PAGE_SIZE;
        Ok(())
    }
    pub fn selected_slice(&self) -> Option<usize> {
        let id = self.selected_slice?;
        self.display_result()
            .is_some_and(|result| result.slices.iter().any(|slice| slice.slice_id == id))
            .then_some(id)
    }
    /// Candidate selection is diagnostic readback only; it cannot grant review
    /// or candidate application.
    pub fn select_candidate(
        &mut self,
        expected: &EditorContext,
        search: &JobTicket,
        id: CandidateId,
    ) -> Result<(), String> {
        if !self.context_matches_current(expected)
            || search.kind != JobKind::Search
            || self.candidate_source.as_ref() != Some(search)
        {
            return Err(
                "STALE_CANDIDATE_SELECTION · search ticket/editor snapshot изменился".into(),
            );
        }
        let Some(result) = self.current_search_result() else {
            return Err("SEARCH_RESULT_NOT_CURRENT".into());
        };
        if !result.candidates.iter().any(|candidate| candidate.id == id) {
            return Err("CANDIDATE_NOT_IN_CURRENT_SEARCH".into());
        }
        self.selected_candidate = Some(id);
        Ok(())
    }
    pub fn selected_candidate(&self) -> Option<CandidateId> {
        let id = self.selected_candidate?;
        self.current_search_result()
            .is_some_and(|result| result.candidates.iter().any(|candidate| candidate.id == id))
            .then_some(id)
    }
    /// Returns only the exact retained main-family fine-pass table for the
    /// currently selected candidate. This is diagnostic readback and does not
    /// grant physical acceptance or candidate application.
    pub fn selected_candidate_table(&self) -> Result<Option<&BoundCandidateTable>, String> {
        let Some(id) = self.selected_candidate else {
            return Ok(None);
        };
        let Some(result) = self.current_search_result() else {
            return Err("SEARCH_RESULT_NOT_CURRENT".into());
        };
        let mut candidates = result
            .candidates
            .iter()
            .filter(|candidate| candidate.id == id);
        let Some(candidate) = candidates.next() else {
            return Err("CANDIDATE_NOT_IN_CURRENT_SEARCH".into());
        };
        if candidates.next().is_some() {
            return Err("AMBIGUOUS_CANDIDATE_ID_IN_CURRENT_SEARCH".into());
        }

        let fine_config = result
            .request
            .public
            .shared_analysis
            .fixed(result.request.public.final_pair.fine_requested);
        let mut matching_table = None;
        for table in &result.retained_tables {
            let surface = &table.evaluation.geometry.circle.surface;
            let matches = table.candidate_id == id
                && table.key == candidate.key
                && table.branch == "main"
                && table.source_fingerprint == result.source_fingerprint
                && table.config == fine_config
                && candidate
                    .final_input_fingerprint
                    .as_ref()
                    .filter(|v| !v.is_empty())
                    == Some(&table.evaluation.input_fingerprint)
                && !table.table_hash.is_empty()
                && !table.evaluation_key.is_empty()
                && surface.center == candidate.center
                && surface.radius_m == candidate.radius_m
                && surface.slide_direction == result.request.public.direction;
            if matches {
                if matching_table.replace(table).is_some() {
                    return Err("AMBIGUOUS_RETAINED_MAIN_FINE_TABLE".into());
                }
            }
        }
        matching_table
            .map(Some)
            .ok_or_else(|| "NO_MATCHING_RETAINED_MAIN_FINE_TABLE".into())
    }
    /// Freshness is not physical acceptance. Use only physically reviewed
    /// geometry for ordinary result overlays; numerical diagnostics stay textual.
    pub fn result_for_overlays(&self) -> Option<&CircularSlopeResult> {
        self.current_result().filter(|result| overlay_result_eligible(result, &self.review, &self.config))
    }
    /// A saved current archive may render only its exact retained tuple and the
    /// same physical/contact gates as a real current result. Unknown worker FP
    /// provenance remains display-only.
    pub fn archived_for_overlays(&self) -> Option<(&CircularAnalysisInput, &CircularAnalysisConfig, &CircularSlopeResult)> {
        if self.job.is_some() || self.current_result().is_some() || self.current_search_result().is_some() { return None; }
        if !self.archive_matches_editor() { return None; }
        let archive = self.archived_view()?;
        if archive.presentation.state != ArchiveState::ArchivedCurrent { return None; }
        let snapshot = archive.snapshot.as_ref()?;
        if !snapshot.archive().bundle.context.saved_calculation_worker_arithmetic.as_ref()
            .is_some_and(|fp| fp.accepted) { return None; }
        let tuple = self.archived_specified()?;
        let review = &self.archive.as_ref()?.review;
        overlay_result_eligible(tuple.2, review, tuple.1).then_some(tuple)
    }
    pub fn saved_input(&self) -> Option<&CircularAnalysisInput> {
        self.bound().then(|| self.saved_input.as_deref()).flatten()
    }
    pub fn effective_flags(&self) -> OverlayFlags {
        let mut flags = self.flags;
        if self.result_for_overlays().is_none() && self.archived_for_overlays().is_none() {
            flags.failure_mass = false;
            flags.slices = false;
            flags.forces = false;
            flags.critical_circle = false;
        }
        flags
    }
    /// Borrow-free catalog pins: IDs only, without cloning a frozen search
    /// request or any Ground payload before the workspace byte audit.
    pub fn catalog_record_ids(&self) -> (Option<DocumentGroundId>, Option<AnalysisCaseId>) {
        if !self
            .binding
            .as_ref()
            .zip(self.live_binding.as_ref())
            .is_some_and(|(saved, live)| saved.same_runtime(live))
        {
            return (None, None);
        }
        (
            self.saved_ground
                .as_ref()
                .map(|ground| ground.ground_section_id),
            self.saved_case.as_ref().map(|case| case.analysis_id),
        )
    }
    pub fn editor_context(&self) -> Result<EditorContext, String> {
        let document = self.live_binding.clone().ok_or("Нет Document/session")?;
        require_runtime(&document)?;
        Ok(EditorContext {
            load_generation: self.load_generation,
            resolved_identity: self.identity.clone(),
            document,
            draft_generation: self.draft_generation,
            ground_generation: self.ground_generation,
            case_generation: self.case_generation,
            search_generation: self.search_generation,
            ground_source: self
                .saved_ground
                .as_ref()
                .map(|g| (g.ground_section_id, g.source_operation_id)),
            case_source: self
                .saved_case
                .as_ref()
                .map(|c| (c.analysis_id, c.source_operation_id)),
            loaded_config: self.case_config_loaded.then_some(self.config),
            frozen_search: self.saved_search_domain.clone(),
        })
    }
    fn record_load_ticket(
        &mut self,
        mode: RecordLoadMode,
        ground_id: DocumentGroundId,
        case_id: Option<AnalysisCaseId>,
    ) -> Result<RecordLoadTicket, String> {
        if self.job.is_some() {
            return Err("BUSY · сначала подтвердите отмену задачи".into());
        }
        if self.catalog_binding.as_ref() != self.live_binding.as_ref()
            || self.catalog_binding.is_none()
        {
            return Err("STALE_CATALOG · обновите каталог текущего Document".into());
        }
        let ground = self
            .grounds
            .iter()
            .find(|g| g.ground_section_id == ground_id)
            .ok_or("Разрез отсутствует в каталоге Document")?;
        let case = case_id
            .map(|id| {
                self.cases
                    .iter()
                    .find(|c| c.analysis_id == id)
                    .ok_or("Case отсутствует в каталоге Document")
            })
            .transpose()?;
        if (mode == RecordLoadMode::Case) != case.is_some()
            || case.is_some_and(|c| c.definition.ground_section_id != ground_id)
        {
            return Err("LOAD_CASE_GROUND_MISMATCH".into());
        }
        let ground_stamp = (ground_id, ground.source_operation_id);
        let case_stamp = case.map(|c| (c.analysis_id, c.source_operation_id));
        let case_config = case.map(|c| c.definition.config);
        self.load_generation = self
            .load_generation
            .checked_add(1)
            .ok_or("LOAD_SEQUENCE_EXHAUSTED")?;
        Ok(RecordLoadTicket {
            context: self.editor_context()?,
            mode,
            ground: ground_stamp,
            case: case_stamp,
            case_config,
        })
    }
    fn check_record_load(
        &self,
        ticket: &RecordLoadTicket,
        live: &DocumentBinding,
        mode: RecordLoadMode,
        ground: &GroundSection,
        case: Option<&CircularCaseEditorProjection>,
    ) -> Result<(), String> {
        let context = self.editor_context()?;
        if self.job.is_some()
            || context != ticket.context
            || &context.document != live
            || ticket.mode != mode
            || ticket.ground != (ground.ground_section_id, ground.source_operation_id)
            || ticket.case != case.map(|c| (c.analysis_id, c.source_operation_id))
            || ticket.case_config != case.map(|c| c.definition.config)
            || case.is_some_and(|c| c.definition.ground_section_id != ground.ground_section_id)
            || !self
                .grounds
                .iter()
                .any(|g| (g.ground_section_id, g.source_operation_id) == ticket.ground)
            || ticket.case.is_some_and(|expected| {
                !self
                    .cases
                    .iter()
                    .any(|c| (c.analysis_id, c.source_operation_id) == expected)
            })
        {
            return Err("STALE_LOAD · запоздалый ответ не заменяет введённый текст; повторите явную загрузку".into());
        }
        validate_ground_record(ground)?;
        if let Some(case) = case {
            validate_case_record(case)?;
        }
        Ok(())
    }
    fn clear_ground_fields(&self, ui: &WidgetRef, cx: &mut Cx) {
        for (_, id) in CIRCULAR_INPUTS {
            if ground_input(*id) {
                ui.text_input(cx, &[*id]).set_text(cx, "");
            }
        }
        for id in [
            id!(gcc_material_selector),
            id!(gcc_interface_selector),
            id!(gcc_point_boundary),
        ] {
            ui.drop_down(cx, &[id]).set_selected_item(cx, 0);
        }
    }
    fn clear_case_fields(&self, ui: &WidgetRef, cx: &mut Cx) {
        for (_, id) in CIRCULAR_INPUTS {
            if !ground_input(*id) && *id != id!(gcc_preview_width) {
                ui.text_input(cx, &[*id]).set_text(cx, "");
            }
        }
        ui.check_box(cx, ids!(gcc_refine))
            .set_active(cx, false, Animate::No);
        ui.check_box(cx, ids!(gcc_search_enabled))
            .set_active(cx, false, Animate::No);
        for id in [
            id!(gcc_circle_mode),
            id!(gcc_direction),
            id!(gcc_search_target),
        ] {
            ui.drop_down(cx, &[id]).set_selected_item(cx, 0);
        }
    }
    pub fn new_ground_draft(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        live: DocumentBinding,
    ) -> Result<(), String> {
        if self.job.is_some() {
            return Err("BUSY · сначала подтвердите отмену задачи".into());
        }
        require_runtime(&live)?;
        if self
            .live_binding
            .as_ref()
            .is_some_and(|old| !old.same_runtime(&live))
        {
            self.grounds.clear();
            self.cases.clear();
            self.catalog_binding = None;
        }
        self.binding = Some(live.clone());
        self.live_binding = Some(live);
        self.stale = false;
        self.saved_ground = None;
        self.saved_case = None;
        self.ground_pick = None;
        self.case_pick = None;
        self.case_ground_ref = None;
        self.draft_ground = Some(GroundDefinition::default());
        self.case_config_loaded = false;
        self.case_dirty = false;
        self.search_dirty = false;
        self.pending_rows.clear();
        self.material_selection = 0;
        self.point_boundary = 0;
        self.interface_selection = 0;
        self.invalidate_source();
        self.clear_ground_fields(ui, cx);
        self.clear_case_fields(ui, cx);
        self.ground_changed();
        self.set_view_fields(ui, cx);
        self.refresh(ui, cx);
        Ok(())
    }
    pub fn load_ground(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        ticket: &RecordLoadTicket,
        live: DocumentBinding,
        ground: Arc<GroundSection>,
    ) -> Result<(), String> {
        self.check_record_load(ticket, &live, RecordLoadMode::Ground, &ground, None)?;
        self.install_ground(ui, cx, live, ground)
    }
    fn install_ground(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        live: DocumentBinding,
        ground: Arc<GroundSection>,
    ) -> Result<(), String> {
        if self.job.is_some() {
            return Err("BUSY · сначала подтвердите отмену задачи".into());
        }
        require_runtime(&live)?;
        validate_ground_record(&ground)?;
        let mut grounds = self.grounds.clone();
        upsert_ground(&mut grounds, ground.clone())?;
        let same_source = self.saved_ground.as_ref().is_some_and(|g| {
            g.ground_section_id == ground.ground_section_id
                && g.source_operation_id == ground.source_operation_id
                && g.definition == ground.definition
        }) && self
            .binding
            .as_ref()
            .is_some_and(|b| b.same_runtime(&live) && b.revision <= live.revision);
        self.binding = Some(live.clone());
        self.live_binding = Some(live);
        self.stale = false;
        if !same_source {
            self.invalidate_source();
        }
        self.clear_ground_fields(ui, cx);
        ui.text_input(cx, ids!(gcc_ground_name))
            .set_text(cx, &ground.name);
        self.ground_pick = Some(ground.ground_section_id);
        if !self.case_config_loaded {
            self.case_ground_ref = Some(ground.ground_section_id);
        }
        self.draft_ground = Some(ground.definition.clone());
        self.saved_ground = Some(ground.clone());
        self.grounds = grounds;
        self.ground_dirty = false;
        self.ground_generation = self.ground_generation.saturating_add(1);
        self.draft_generation = self.draft_generation.saturating_add(1);
        self.pending_rows.clear();
        self.update_dirty();
        self.material_selection = 0;
        self.point_boundary = 0;
        self.interface_selection = 0;
        self.status = "GROUND LOADED · разрез самостоятельный; case не создаётся".into();
        self.set_view_fields(ui, cx);
        self.refresh(ui, cx);
        Ok(())
    }
    pub fn new_case_draft(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        ticket: &RecordLoadTicket,
        live: DocumentBinding,
        config: CircularAnalysisConfig,
        basis: AnalysisBasis,
        units: InputUnits,
    ) -> Result<(), String> {
        require_runtime(&live)?;
        validate_config(&config).map_err(|e| e.to_string())?;
        if units != InputUnits::default() || self.ground_dirty || self.job.is_some() {
            return Err(
                "Сначала сохраните разрез; нужны поддерживаемые units и свободная задача".into(),
            );
        }
        let ground_id = ticket.ground.0;
        let ground = self
            .grounds
            .iter()
            .find(|g| g.ground_section_id == ground_id)
            .cloned()
            .ok_or("Выберите существующий разрез")?;
        self.check_record_load(ticket, &live, RecordLoadMode::NewCase, &ground, None)?;
        self.install_ground(ui, cx, live, ground)?;
        self.clear_case_fields(ui, cx);
        self.saved_case = None;
        self.case_pick = None;
        self.case_ground_ref = Some(ground_id);
        self.config = config;
        self.basis = basis;
        self.units = units;
        self.case_config_loaded = true;
        self.invalidate_source();
        self.search_dirty = false;
        self.changed();
        self.hydrate_config_fields(ui, cx);
        self.status =
            "NEW CASE · укажите локальный slip ID, окружность, имя и ширину; ID назначит Document"
                .into();
        self.refresh(ui, cx);
        Ok(())
    }
    pub fn load_case(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        ticket: &RecordLoadTicket,
        live: DocumentBinding,
        ground: Arc<GroundSection>,
        case: Arc<CircularCaseEditorProjection>,
    ) -> Result<(), String> {
        require_runtime(&live)?;
        validate_ground_record(&ground)?;
        validate_case_record(&case)?;
        let mut cases = self.cases.clone();
        upsert_case(&mut cases, case.clone())?;
        if case.definition.ground_section_id != ground.ground_section_id {
            return Err("CASE_GROUND_REFERENCE_MISMATCH".into());
        }
        self.check_record_load(ticket, &live, RecordLoadMode::Case, &ground, Some(&case))?;
        self.install_ground(ui, cx, live, ground)?;
        self.clear_case_fields(ui, cx);
        self.config = case.definition.config;
        self.basis = case.definition.basis;
        self.units = case.definition.units;
        self.case_config_loaded = true;
        self.case_ground_ref = Some(case.definition.ground_section_id);
        self.case_pick = Some(case.analysis_id);
        for (id, value) in [
            (id!(gcc_case_name), case.name.clone()),
            (
                id!(gcc_slip_id),
                case.definition.slip_surface.slip_id.0.clone(),
            ),
            (
                id!(gcc_center_x),
                case.definition.slip_surface.center.x.to_string(),
            ),
            (
                id!(gcc_center_z),
                case.definition.slip_surface.center.z.to_string(),
            ),
            (
                id!(gcc_radius),
                case.definition.slip_surface.radius_m.to_string(),
            ),
            (
                id!(gcc_width),
                case.definition.extrusion_width_m.to_string(),
            ),
        ] {
            ui.text_input(cx, &[id]).set_text(cx, &value);
        }
        ui.drop_down(cx, ids!(gcc_direction)).set_selected_item(
            cx,
            usize::from(case.definition.slip_surface.slide_direction == SlideDirection::NegativeX),
        );
        self.saved_case = Some(case.clone());
        self.cases = cases;
        self.invalidate_source();
        self.case_dirty = false;
        self.search_dirty = false;
        self.case_generation = self.case_generation.saturating_add(1);
        self.draft_generation = self.draft_generation.saturating_add(1);
        self.update_dirty();
        self.hydrate_config_fields(ui, cx);
        self.status="CASE LOADED · требуются owner resolved snapshot/fingerprint и host gates; расчёт NOT_RUN".into();
        self.refresh(ui, cx);
        Ok(())
    }
    /// The shared owner resolves records; this method never computes a fingerprint,
    /// constructs slices, solves or writes a Document/history.
    pub fn bind_resolved(
        &mut self,
        expected: &EditorContext,
        resolved: Arc<ResolvedCircularAnalysis>,
        input_fingerprint: String,
    ) -> Result<(), String> {
        if self.editor_context()? != *expected {
            return Err("STALE_RESOLVE · snapshot другого сеанса/редактора не применяется".into());
        }
        if !self.bound() || self.ground_dirty || self.case_dirty {
            return Err("Сначала сохраните/загрузите актуальные ground и case".into());
        }
        let ground = self
            .saved_ground
            .as_ref()
            .ok_or("Нет сохранённого GroundSection")?;
        let case = self.saved_case.as_ref().ok_or("Нет сохранённого case")?;
        validate_resolved(ground, case, &resolved)?;
        if input_fingerprint.trim().is_empty() {
            return Err("Не передан owner input/config fingerprint".into());
        }
        self.config = resolved.config;
        self.saved_input = Some(Arc::new(resolved.input.clone()));
        let identity = CalculationIdentity {
            document: self.binding.clone().unwrap(),
            ground_id: ground.ground_section_id,
            analysis_id: case.analysis_id,
            ground_source_operation_id: resolved.ground_source_operation_id,
            case_source_operation_id: resolved.case_source_operation_id,
            config: resolved.config,
            source_revision: resolved.input.source_revision.clone(),
            input_fingerprint,
            source_fingerprint: source_fingerprint(),
            solver_version: CIRCULAR_SOLVER_VERSION.into(),
        };
        if self
            .identity
            .as_ref()
            .is_none_or(|old| !old.same_source(&identity))
        {
            self.analysis_gate_accepted = false;
            self.search_gate_accepted = false;
        }
        self.identity = Some(identity);
        // Binding source does not grant either gate.
        Ok(())
    }
    fn edited_config(&self, ui: &WidgetRef, cx: &mut Cx) -> Result<CircularAnalysisConfig, String> {
        if !self.case_config_loaded {
            return Err("Полный config case не загружен".into());
        }
        let mut config = self.config;
        config.slices.target_slices = integer(ui, cx, id!(gcc_slice_count), "Число отсеков")?;
        config.slices.verify_refinement = ui.check_box(cx, ids!(gcc_refine)).active(cx);
        validate_config(&config).map_err(|e| e.to_string())?;
        Ok(config)
    }
    fn hydrate_config_fields(&self, ui: &WidgetRef, cx: &mut Cx) {
        ui.text_input(cx, ids!(gcc_slice_count))
            .set_text(cx, &self.config.slices.target_slices.to_string());
        ui.check_box(cx, ids!(gcc_refine)).set_active(
            cx,
            self.config.slices.verify_refinement,
            Animate::No,
        );
    }
    pub fn set_case_config(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        expected: &EditorContext,
        config: CircularAnalysisConfig,
    ) -> Result<(), String> {
        if self.editor_context()? != *expected {
            return Err("STALE_CONFIG_EDITOR · ответ не заменяет новый ввод".into());
        }
        if !self.case_config_loaded || !self.bound() {
            return Err("Нет case для полного config".into());
        }
        validate_config(&config).map_err(|e| e.to_string())?;
        if self.config != config {
            self.config = config;
            self.changed();
        }
        self.hydrate_config_fields(ui, cx);
        self.refresh(ui, cx);
        Ok(())
    }
    pub fn save_ground_proposal(
        &self,
        ui: &WidgetRef,
        cx: &mut Cx,
    ) -> Result<SaveGroundProposal, String> {
        if !self.bound() || self.job.is_some() {
            return Err("STALE/BUSY · сохранение разреза недоступно".into());
        }
        if !self.pending_rows.is_empty() {
            return Err("UNAPPLIED_ROW · сначала примените локальные строки разреза".into());
        }
        let definition = self.draft_ground.clone().ok_or("Нет черновика разреза")?;
        inspect_ground_readiness(&definition).map_err(|e| e.to_string())?; // Draft is valid; no circle/config required.
        let name = text(ui, cx, id!(gcc_ground_name), MAX_ID)?;
        let expected_ground = self
            .saved_ground
            .as_ref()
            .map(|g| (g.ground_section_id, g.source_operation_id));
        let command = if let Some(g) = &self.saved_ground {
            EngineeringCommand::UpdateGround {
                ground_section_id: g.ground_section_id,
                definition,
                name: Some(name),
            }
        } else {
            EngineeringCommand::CreateGround { definition, name }
        };
        Ok(SaveGroundProposal {
            context: self.editor_context()?,
            expected: self.binding.clone().unwrap(),
            generation: self.ground_generation,
            expected_ground,
            before: self.saved_ground.clone(),
            command,
        })
    }
    pub fn save_case_proposal(
        &self,
        ui: &WidgetRef,
        cx: &mut Cx,
    ) -> Result<SaveCaseProposal, String> {
        if !self.bound() || self.job.is_some() || self.ground_dirty || !self.case_config_loaded {
            return Err("Сначала сохраните разрез и загрузите полный config case".into());
        }
        if self.catalog_binding.as_ref() != self.live_binding.as_ref()
            || self.catalog_binding.is_none()
        {
            return Err("STALE_CATALOG · обновите каталог текущего Document".into());
        }
        let id = self
            .case_ground_ref
            .ok_or("Нужна явная ссылка на существующий GroundSection")?;
        let ground = self
            .grounds
            .iter()
            .find(|g| g.ground_section_id == id)
            .ok_or("GroundSection отсутствует в текущем Document")?;
        let config = self.edited_config(ui, cx)?;
        let mut slip_surface = self.circle(ui, cx, &config)?;
        if let Some(old) = &self.saved_case {
            let s = &old.definition.slip_surface;
            if s.slip_id == slip_surface.slip_id
                && s.center == slip_surface.center
                && s.radius_m == slip_surface.radius_m
                && s.slide_direction == slip_surface.slide_direction
            {
                slip_surface.selected_endpoints = s.selected_endpoints;
            }
        }
        let definition = CircularCaseDefinition {
            ground_section_id: id,
            slip_surface,
            basis: self.basis,
            units: self.units,
            extrusion_width_m: positive(ui, cx, id!(gcc_width), "Ширина экструзии B")?,
            config,
            search_spec: match self.saved_case.as_ref() {
                Some(case) => case.definition.search_spec.clone(),
                None => None,
            },
        };
        let name = text(ui, cx, id!(gcc_case_name), MAX_ID)?;
        let command = if let Some(c) = &self.saved_case {
            EngineeringCommand::UpdateCircular {
                analysis_id: c.analysis_id,
                definition,
                name: Some(name),
            }
        } else {
            EngineeringCommand::CreateCircular { definition, name }
        };
        Ok(SaveCaseProposal {
            context: self.editor_context()?,
            expected: self.binding.clone().unwrap(),
            generation: self.case_generation,
            ground_generation: self.ground_generation,
            expected_case: self
                .saved_case
                .as_ref()
                .map(|c| (c.analysis_id, c.source_operation_id)),
            before: self.saved_case.clone(),
            referenced_ground: ground.clone(),
            expected_ground: (id, ground.source_operation_id),
            command,
        })
    }
    /// Save only the explicit search policy through the owner's ordinary
    /// UpdateCircular transaction. Circle, config and ground stay byte-for-byte
    /// from the current saved record.
    pub fn save_search_proposal(
        &self,
        ui: &WidgetRef,
        cx: &mut Cx,
    ) -> Result<SaveCaseProposal, String> {
        if !self.bound()
            || self.job.is_some()
            || self.ground_dirty
            || self.case_dirty
            || !self.pending_rows.is_empty()
            || !self.case_config_loaded
            || !ui.check_box(cx, ids!(gcc_search_enabled)).active(cx)
        {
            return Err(
                "Сначала сохраните ground/case и все правки; отдельно включите поиск".into(),
            );
        }
        if self.catalog_binding.as_ref() != self.live_binding.as_ref()
            || self.catalog_binding.is_none()
        {
            return Err("STALE_CATALOG · обновите каталог текущего Document".into());
        }
        let case = self
            .saved_case
            .as_ref()
            .ok_or("SEARCH_CASE_NOT_SAVED · сначала сохраните новый case без политики поиска")?;
        let ground = self
            .saved_ground
            .as_ref()
            .ok_or("Нет сохранённого GroundSection для текущего case")?;
        if !self.ready_for_analysis()
            || case.definition.ground_section_id != ground.ground_section_id
            || self.case_ground_ref != Some(ground.ground_section_id)
            || self.config != case.definition.config
        {
            return Err(
                "SEARCH_CASE_SOURCE_MISMATCH · перечитайте актуальный case и config".into(),
            );
        }
        let input = self
            .saved_input
            .as_deref()
            .ok_or("SEARCH_CASE_NOT_RESOLVED · сначала разрешите сохранённый case")?;
        let draft = search_box(
            ui,
            cx,
            &case.definition.config,
            input,
            self.search_request_template.as_ref(),
        )?;
        if !search_matches_input(&draft.request, input) {
            return Err(
                "SEARCH_TEMPLATE_BINDING_MISMATCH · запрос другого case/source не сохраняется"
                    .into(),
            );
        }
        if draft.request.shared_analysis != search_analysis_config(&case.definition.config) {
            return Err("SEARCH_CONFIG_MISMATCH · лимит отсеков и shared settings должны совпадать с config case; сначала измените config".into());
        }
        let search_spec = circular_search_spec(&draft.request);
        if case.definition.search_spec.as_ref() == Some(&search_spec) {
            return Err("SEARCH_POLICY_UNCHANGED · сохранённая политика уже совпадает".into());
        }
        let mut definition = case.definition.clone();
        definition.search_spec = Some(search_spec);
        let command = EngineeringCommand::UpdateCircular {
            analysis_id: case.analysis_id,
            definition,
            name: None,
        };
        Ok(SaveCaseProposal {
            context: self.editor_context()?,
            expected: self.binding.clone().ok_or("Нет Document binding")?,
            generation: self.case_generation,
            expected_case: Some((case.analysis_id, case.source_operation_id)),
            ground_generation: self.ground_generation,
            before: Some(case.clone()),
            referenced_ground: ground.clone(),
            expected_ground: (ground.ground_section_id, ground.source_operation_id),
            command,
        })
    }
    pub fn accept_ground_saved(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        proposal: &SaveGroundProposal,
        command_id: &CommandId,
        receipt: &CommitReceipt,
        live: DocumentBinding,
        ground: Arc<GroundSection>,
    ) -> Result<(), String> {
        check_receipt(&proposal.expected, &live, command_id, receipt)?;
        if self.editor_context()? != proposal.context
            || self.ground_generation != proposal.generation
            || !self
                .live_binding
                .as_ref()
                .is_some_and(|b| b.same_runtime(&live) && b.revision <= live.revision)
            || !ground_command_matches(proposal, &ground, receipt)
        {
            return Err("GROUND_SAVE_RECEIPT_CONFLICT · новые данные не заменяют черновик".into());
        }
        validate_ground_record(&ground)?;
        let mut grounds = self.grounds.clone();
        upsert_ground(&mut grounds, ground.clone())?;
        let same = self.saved_ground.as_ref().is_some_and(|g| {
            g.ground_section_id == ground.ground_section_id
                && g.source_operation_id == ground.source_operation_id
                && g.definition == ground.definition
        });
        if !same {
            self.invalidate_source();
        }
        self.binding = Some(live.clone());
        self.live_binding = Some(live.clone());
        self.stale = false;
        self.draft_ground = Some(ground.definition.clone());
        self.ground_pick = Some(ground.ground_section_id);
        self.saved_ground = Some(ground.clone());
        self.grounds = grounds;
        self.catalog_binding = if live.revision == receipt.revision_after {
            Some(live.clone())
        } else {
            None
        };
        if !self.case_config_loaded {
            self.case_ground_ref = Some(ground.ground_section_id);
        }
        self.ground_generation = self.ground_generation.saturating_add(1);
        self.draft_generation = self.draft_generation.saturating_add(1);
        self.ground_dirty = false;
        self.pending_rows.clear();
        self.update_dirty();
        if let Some(i) = &mut self.identity {
            i.document = live;
        }
        ui.text_input(cx, ids!(gcc_ground_name))
            .set_text(cx, &ground.name);
        self.status =
            "GROUND SAVED · source stamp выдан Document; готовность определена owner".into();
        self.refresh(ui, cx);
        Ok(())
    }
    pub fn accept_case_saved(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        proposal: &SaveCaseProposal,
        command_id: &CommandId,
        receipt: &CommitReceipt,
        live: DocumentBinding,
        ground: Arc<GroundSection>,
        case: Arc<CircularCaseEditorProjection>,
    ) -> Result<(), String> {
        check_receipt(&proposal.expected, &live, command_id, receipt)?;
        if self.editor_context()? != proposal.context
            || self.case_generation != proposal.generation
            || self.ground_generation != proposal.ground_generation
            || self.ground_dirty
            || !self
                .live_binding
                .as_ref()
                .is_some_and(|b| b.same_runtime(&live) && b.revision <= live.revision)
            || (ground.ground_section_id, ground.source_operation_id) != proposal.expected_ground
            || ground.definition != proposal.referenced_ground.definition
            || case.definition.ground_section_id != ground.ground_section_id
            || !case_command_matches(proposal, &case, receipt)
        {
            return Err("CASE_SAVE_RECEIPT_CONFLICT · новые данные не заменяют черновик".into());
        }
        validate_ground_record(&ground)?;
        validate_case_record(&case)?;
        let mut grounds = self.grounds.clone();
        upsert_ground(&mut grounds, ground.clone())?;
        let mut cases = self.cases.clone();
        upsert_case(&mut cases, case.clone())?;
        let same = self.saved_case.as_ref().is_some_and(|c| {
            c.analysis_id == case.analysis_id
                && c.source_operation_id == case.source_operation_id
                && c.definition == case.definition
        }) && self.saved_ground.as_ref().is_some_and(|g| {
            g.ground_section_id == ground.ground_section_id
                && g.source_operation_id == ground.source_operation_id
                && g.definition == ground.definition
        });
        if !same {
            self.invalidate_source();
        }
        self.binding = Some(live.clone());
        self.live_binding = Some(live.clone());
        self.stale = false;
        self.saved_case = Some(case.clone());
        self.cases = cases;
        self.saved_ground = Some(ground.clone());
        self.draft_ground = Some(ground.definition.clone());
        self.ground_pick = Some(ground.ground_section_id);
        ui.text_input(cx, ids!(gcc_ground_name))
            .set_text(cx, &ground.name);
        self.grounds = grounds;
        self.catalog_binding = if live.revision == receipt.revision_after {
            Some(live.clone())
        } else {
            None
        };
        self.case_generation = self.case_generation.saturating_add(1);
        self.draft_generation = self.draft_generation.saturating_add(1);
        self.case_pick = Some(case.analysis_id);
        self.case_ground_ref = Some(case.definition.ground_section_id);
        self.config = case.definition.config;
        self.case_config_loaded = true;
        self.case_dirty = false;
        self.update_dirty();
        if let Some(i) = &mut self.identity {
            i.document = live;
        }
        ui.text_input(cx, ids!(gcc_case_name))
            .set_text(cx, &case.name);
        self.hydrate_config_fields(ui, cx);
        self.status = "CASE SAVED · только ссылка на GroundSection, без копии геологии".into();
        self.refresh(ui, cx);
        Ok(())
    }
    /// Freeze the current complete request for this session. Persisting a saved
    /// search policy remains a separate explicit SaveSearchSettings operation.
    pub fn freeze_search_settings(&mut self, ui: &WidgetRef, cx: &mut Cx) -> Result<(), String> {
        if !self.ready_for_analysis() || self.job.is_some() {
            return Err("Нужны сохранённые ready ground/case и owner resolved snapshot".into());
        }
        self.saved_search_domain = if ui.check_box(cx, ids!(gcc_search_enabled)).active(cx) {
            Some(search_box(
                ui,
                cx,
                &self.config,
                self.saved_input.as_deref().unwrap(),
                self.search_request_template.as_ref(),
            )?)
        } else {
            None
        };
        self.search_generation = self.search_generation.saturating_add(1);
        self.search_gate_accepted = false;
        self.search_dirty = false;
        self.update_dirty();
        self.search_result = None;
        self.candidate_source = None;
        self.selected_candidate = None;
        self.status = "SEARCH PREVIEW FROZEN · только текущий сеанс; для записи в case нажмите «Сохранить настройки поиска»".into();
        self.refresh(ui, cx);
        Ok(())
    }
    pub fn preview_width(&self) -> Option<f64> {
        self.preview_width
    }
    fn set_view_fields(&self, ui: &WidgetRef, cx: &mut Cx) {
        for (id, value) in [
            (id!(gcc_ground), self.flags.ground),
            (id!(gcc_layers), self.flags.soil_layers),
            (id!(gcc_mass), self.flags.failure_mass),
            (id!(gcc_slices), self.flags.slices),
            (id!(gcc_forces), self.flags.forces),
            (id!(gcc_circle), self.flags.critical_circle),
        ] {
            ui.check_box(cx, &[id]).set_active(cx, value, Animate::No);
        }
    }
    fn set_search_fields(&self, ui: &WidgetRef, cx: &mut Cx, domain: Option<&SearchBox>) {
        ui.check_box(cx, ids!(gcc_search_enabled))
            .set_active(cx, domain.is_some(), Animate::No);
        ui.drop_down(cx, ids!(gcc_search_target)).set_selected_item(
            cx,
            domain.map_or(0, |d| match d.request.target_method {
                TargetMethod::Fellenius => 1,
                TargetMethod::Bishop => 2,
                TargetMethod::Both => 3,
            }),
        );
        if let Some(d) = domain {
            let r = &d.request;
            for (id, n) in [
                (id!(gcc_xmin), r.domain.center_x.min),
                (id!(gcc_xmax), r.domain.center_x.max),
                (id!(gcc_zmin), r.domain.center_z.min),
                (id!(gcc_zmax), r.domain.center_z.max),
                (id!(gcc_rmin), r.domain.radius.min),
                (id!(gcc_rmax), r.domain.radius.max),
            ] {
                ui.text_input(cx, &[id]).set_text(cx, &n.to_string());
            }
            for (id, n) in [
                (id!(gcc_xintervals), u64::from(r.domain.center_x.intervals)),
                (id!(gcc_zintervals), u64::from(r.domain.center_z.intervals)),
                (id!(gcc_rintervals), u64::from(r.domain.radius.intervals)),
                (id!(gcc_candidates_limit), r.max_candidates),
                (id!(gcc_time_limit), r.max_wall_time_ms),
                (id!(gcc_search_levels), u64::from(r.refinement.levels)),
                (
                    id!(gcc_search_neighborhoods),
                    u64::from(r.refinement.promising_per_method),
                ),
                (
                    id!(gcc_search_max_slices),
                    r.shared_analysis.limits.max_slices as u64,
                ),
            ] {
                ui.text_input(cx, &[id]).set_text(cx, &n.to_string());
            }
            for (range, lo, hi) in [
                (
                    r.domain.allowed_entry_x,
                    id!(gcc_entry_xmin),
                    id!(gcc_entry_xmax),
                ),
                (
                    r.domain.allowed_exit_x,
                    id!(gcc_exit_xmin),
                    id!(gcc_exit_xmax),
                ),
            ] {
                ui.text_input(cx, &[lo])
                    .set_text(cx, &range.map_or(String::new(), |v| v.min_m.to_string()));
                ui.text_input(cx, &[hi])
                    .set_text(cx, &range.map_or(String::new(), |v| v.max_m.to_string()));
            }
        }
    }
    /// Bind the full request returned by Document::resolve_circular_search.
    /// The owner supplies the actual resolver result; this method never builds
    /// or weakens a persisted policy.
    pub fn set_search_request_template(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        expected: &EditorContext,
        resolved: ResolvedCircularSearch,
    ) -> Result<(), String> {
        if self.editor_context()? != *expected {
            return Err("STALE_SEARCH_TEMPLATE · другой сеанс/редактор".into());
        }
        if !self.ready_for_analysis() || self.ground_dirty || self.case_dirty {
            return Err("Сначала разрешите актуальный сохранённый ground/case".into());
        }
        let ground = self
            .saved_ground
            .as_deref()
            .ok_or("Нет сохранённого GroundSection")?;
        let case = self.saved_case.as_deref().ok_or("Нет сохранённого case")?;
        let input = self
            .saved_input
            .as_ref()
            .ok_or("SEARCH_CASE_NOT_RESOLVED · сначала разрешите сохранённый case")?;
        let saved_spec =
            case.definition.search_spec.as_ref().ok_or(
                "SEARCH_SPEC_NOT_CONFIGURED · сначала настройте и сохраните политику поиска",
            )?;
        if !self.bound()
            || self.config != case.definition.config
            || resolved.ground_source_operation_id != ground.source_operation_id
            || resolved.case_source_operation_id != case.source_operation_id
            || resolved.request.search_id != case.analysis_id.to_string()
            || !search_matches_input(&resolved.request, input)
            || resolved.request.shared_analysis != search_analysis_config(&case.definition.config)
            || circular_search_spec(&resolved.request) != *saved_spec
        {
            return Err("SEARCH_TEMPLATE_BINDING_MISMATCH · resolver result must match the saved case policy and both source stamps".into());
        }
        let domain = SearchBox {
            request: resolved.request.clone(),
            analysis_config: case.definition.config,
        };
        if let Some(error) = persisted_search_ui_limit_error(&domain.request) {
            return Err(error);
        }
        validate_search_box(&domain).map_err(|error| {
            format!("SEARCH_POLICY_UI_PRECHECK_FAILED · Core-saved values were preserved: {error}")
        })?;
        let unchanged = self.resolved_search.as_ref() == Some(&resolved)
            && self.search_request_template.as_ref() == Some(&resolved.request);
        self.resolved_search = Some(resolved.clone());
        if !unchanged {
            self.search_request_template = Some(resolved.request.clone());
            self.saved_search_domain = None;
            self.search_settings_changed();
            self.set_search_fields(ui, cx, Some(&domain));
        }
        self.refresh(ui, cx);
        Ok(())
    }
    /// Install a complete, explicit draft request supplied by the host's policy
    /// editor. It is source-bound but never counts as Document resolution.
    pub fn set_search_draft_template(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        expected: &EditorContext,
        request: SearchPublicRequestV1,
    ) -> Result<(), String> {
        if self.editor_context()? != *expected {
            return Err("STALE_SEARCH_DRAFT · другой сеанс/редактор".into());
        }
        if !self.ready_for_analysis()
            || self.job.is_some()
            || self.ground_dirty
            || self.case_dirty
            || !self.pending_rows.is_empty()
        {
            return Err("Сначала сохраните правки и разрешите актуальный ground/case".into());
        }
        let case = self.saved_case.as_deref().ok_or("Нет сохранённого case")?;
        let input = self
            .saved_input
            .as_deref()
            .ok_or("SEARCH_CASE_NOT_RESOLVED · сначала разрешите сохранённый case")?;
        if self.config != case.definition.config
            || request.search_id != case.analysis_id.to_string()
            || !search_matches_input(&request, input)
        {
            return Err("SEARCH_DRAFT_BINDING_MISMATCH · запрос должен ссылаться на текущие case/ground/source".into());
        }
        let domain = SearchBox {
            request: request.clone(),
            analysis_config: case.definition.config,
        };
        validate_search_box(&domain)?;
        self.search_request_template = Some(request);
        self.resolved_search = None;
        self.saved_search_domain = None;
        self.search_settings_changed();
        self.set_search_fields(ui, cx, Some(&domain));
        self.refresh(ui, cx);
        Ok(())
    }
    fn circle(
        &self,
        ui: &WidgetRef,
        cx: &mut Cx,
        config: &CircularAnalysisConfig,
    ) -> Result<CircularSlipSurface, String> {
        let id = CircularSlipId(text(ui, cx, id!(gcc_slip_id), MAX_ID)?);
        let direction = if ui.drop_down(cx, ids!(gcc_direction)).selected_item() == 0 {
            SlideDirection::PositiveX
        } else {
            SlideDirection::NegativeX
        };
        if ui.drop_down(cx, ids!(gcc_circle_mode)).selected_item() == 0 {
            CircularSlipSurface::from_center_radius(
                id,
                Point2::new(
                    number(ui, cx, id!(gcc_center_x), "X центра")?,
                    number(ui, cx, id!(gcc_center_z), "Z центра")?,
                ),
                positive(ui, cx, id!(gcc_radius), "Радиус")?,
                direction,
            )
        } else {
            CircularSlipSurface::from_three_points(
                id,
                [
                    Point2::new(
                        number(ui, cx, id!(gcc_p1x), "X1")?,
                        number(ui, cx, id!(gcc_p1z), "Z1")?,
                    ),
                    Point2::new(
                        number(ui, cx, id!(gcc_p2x), "X2")?,
                        number(ui, cx, id!(gcc_p2z), "Z2")?,
                    ),
                    Point2::new(
                        number(ui, cx, id!(gcc_p3x), "X3")?,
                        number(ui, cx, id!(gcc_p3z), "Z3")?,
                    ),
                ],
                direction,
                config,
            )
        }
        .map_err(|e| e.to_string())
    }
    fn begin_job(&mut self, kind: JobKind) -> Result<JobTicket, String> {
        // Diagnostics read this value; only the ordinary action allocates it.
        if !self.ready_for_analysis() || self.job.is_some() {
            return Err("Нужны сохранённые ready ground + case, оба source stamps и owner resolved snapshot".into());
        }
        if (kind == JobKind::Analyze && !self.analysis_gate_accepted)
            || (kind == JobKind::Search && !self.search_gate_accepted)
        {
            return Err("HOST_GATE_NOT_ACCEPTED".into());
        }
        if kind == JobKind::Search && !self.search_dispatch_ready() {
            return Err("SEARCH_SPEC_NOT_FROZEN/NOT_CURRENT · сохраните политику, разрешите её из Document и зафиксируйте точный запрос".into());
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("UI job sequence exhausted")?;
        if kind == JobKind::Search {
            self.search_generation = self.search_generation.saturating_add(1);
        }
        let ticket = JobTicket {
            sequence: self.sequence,
            search_generation: self.search_generation,
            kind,
            identity: self.identity.clone().unwrap(),
            search_spec: if kind == JobKind::Search {
                self.saved_search_domain.clone()
            } else {
                None
            },
        };
        self.job = Some(ticket.clone());
        self.result = None;
        self.result_source = None;
        self.selected_slice = None;
        self.review = ResultReview::default();
        self.search_result = None;
        self.candidate_source = None;
        self.selected_candidate = None;
        self.cancelling = false;
        self.status = "RUNNING · immutable ground/case/config/spec/mode snapshot".into();
        Ok(ticket)
    }
    fn job_current(&self, ticket: &JobTicket) -> bool {
        self.job.as_ref() == Some(ticket)
            && self.ready_for_analysis()
            && !self.cancelling
            && self
                .identity
                .as_ref()
                .is_some_and(|i| i.same_source(&ticket.identity))
            && (ticket.kind != JobKind::Search
                || (ticket.search_generation == self.search_generation
                    && ticket.search_spec == self.saved_search_domain
                    && self.search_dispatch_ready()))
    }
    pub(super) fn diagnostic_next_job_sequence(&self)->Option<u64>{self.sequence.checked_add(1)}
    fn reject_finished_job(&mut self, ticket: &JobTicket) {
        if self.job.as_ref() == Some(ticket) {
            self.job = None;
            self.cancelling = false;
            self.status =
                "STALE_RESULT · завершившаяся задача не соответствует текущему черновику/поиску"
                    .into();
        }
    }
    pub fn accept_analysis(
        &mut self,
        ticket: &JobTicket,
        result: Arc<CircularSlopeResult>,
        mut review: ResultReview,
    ) -> Result<(), String> {
        if ticket.kind != JobKind::Analyze
            || !self.job_current(ticket)
            || !ticket.identity.matches(&result)
        {
            self.reject_finished_job(ticket);
            return Err("STALE_RESULT · ответ старой задачи не опубликован".into());
        }
        // Contact status comes from the core, never from numerical convergence
        // or an optimistic host badge. A diagnostic root may have N_B < 0.
        let bishop = &result.bishop_result;
        review.finite_table.physical_status = match bishop.physical_assessment.status {
            BishopPhysicalStatus::Admissible => PhysicalStatus::Admissible,
            BishopPhysicalStatus::NumericallyIndeterminate => PhysicalStatus::NotAssessed,
            _ => PhysicalStatus::Inadmissible,
        };
        review.finite_table.explanation = format!(
            "Bishop finite table: {:?}; min N={}",
            bishop.physical_assessment.status, bishop.physical_assessment.minimum_normal_kn_per_m
        );
        if bishop.physical_assessment.status != BishopPhysicalStatus::Admissible
            || bishop.convergence_status != BishopConvergenceStatus::Converged
            || bishop.accepted_factor_of_safety.is_none()
        {
            review.bishop.physical_status = PhysicalStatus::Inadmissible;
            review.bishop.explanation = format!(
                "Core: {:?}; accepted Fs={:?}; min N_B={} кН/м",
                bishop.physical_assessment.status,
                bishop.accepted_factor_of_safety,
                bishop.physical_assessment.minimum_normal_kn_per_m
            );
        }
        match bishop.whole_arc_contact.as_ref() {
            None => {
                review.whole_arc.physical_status = PhysicalStatus::NotAssessed;
                review.whole_arc_applicability = None;
                review.whole_arc.explanation = "Core whole arc: NOT_ASSESSED".into();
            }
            Some(r) => {
                let bound =
                    declared_contact_is_bound(r, &result.source_fingerprint, &self.config.contact)
                        && review.whole_arc_applicability.as_deref() == Some(r.scope.as_str());
                if !bound {
                    review.whole_arc.physical_status =
                        if r.status == WholeArcContactStatus::Violation {
                            PhysicalStatus::Inadmissible
                        } else {
                            PhysicalStatus::NotAssessed
                        };
                }
                review.whole_arc.explanation = format!(
                    "Core: {:?}; coverage={}; scope={}; root range={}; independent={}; critical_eligible={}; declared model only",
                    r.status,
                    r.coverage_complete,
                    r.scope,
                    r.root_range_scope,
                    r.independent_whole_arc_acceptance,
                    r.eligible_for_critical_minimum
                );
            }
        }
        self.result = Some(result);
        self.result_source = Some(ticket.clone());
        self.selected_slice = None;
        match self
            .result
            .as_ref()
            .unwrap()
            .bishop_result
            .endpoint_assessment
            .as_ref()
        {
            None => {
                review.geometry_endpoints.physical_status = PhysicalStatus::NotAssessed;
                review.geometry_endpoints.explanation =
                    "Core geometry endpoint assessment: UNKNOWN / NOT_ASSESSED".into();
            }
            Some(assessment) if !assessment.passed => {
                review.geometry_endpoints.physical_status = PhysicalStatus::Inadmissible;
                review.geometry_endpoints.explanation = assessment
                    .endpoints
                    .iter()
                    .map(|e| format!("{:?}: {:?}; {}", e.endpoint_type, e.status, e.reason))
                    .collect::<Vec<_>>()
                    .join("; ");
            }
            _ => {}
        }
        self.review = review;
        self.job = None;
        self.page = 0;
        self.status = "COMPLETED · физическая допустимость и верификация показаны отдельно".into();
        Ok(())
    }
    pub fn finish_error(&mut self, ticket: &JobTicket, message: &str) {
        if self.job.as_ref() == Some(ticket) {
            self.job = None;
            if ticket.kind == JobKind::Analyze {
                self.result = None;
                self.result_source = None;
            }
            self.status = format!(
                "{} · {}",
                if self.cancelling {
                    "CANCELLED"
                } else {
                    "FAILED"
                },
                bounded(message, MAX_TEXT)
            );
            self.cancelling = false;
        }
    }
    /// Drop only host-owned work/results tied to this exact ticket. This is an
    /// eviction/cancellation path, not a synthetic completion or source change.
    pub fn discard_host_result(&mut self, ticket: &JobTicket, message: &str) {
        let active_match = self.job.as_ref() == Some(ticket);
        let analysis_match =
            ticket.kind == JobKind::Analyze && self.result_source.as_ref() == Some(ticket);
        let search_match =
            ticket.kind == JobKind::Search && self.candidate_source.as_ref() == Some(ticket);
        if !active_match && !analysis_match && !search_match {
            return;
        }
        if active_match {
            self.job = None;
            self.cancelling = false;
        }
        if analysis_match {
            self.result = None;
            self.result_source = None;
            self.review = ResultReview::default();
            self.selected_slice = None;
        }
        if search_match {
            self.search_result = None;
            self.candidate_source = None;
            self.selected_candidate = None;
        }
        self.status = format!("HOST_RESULT_DISCARDED · {}", bounded(message, MAX_TEXT));
    }
    /// Legacy row publication cannot create an accepted search.
    pub fn accept_candidates(
        &mut self,
        ticket: &JobTicket,
        _rows: Vec<CandidateSummary>,
    ) -> Result<(), String> {
        if ticket.kind != JobKind::Search || !self.job_current(ticket) {
            self.reject_finished_job(ticket);
            return Err("STALE_SEARCH".into());
        }
        self.finish_error(ticket, "Нужен типизированный SearchResultV1");
        Err("UNTYPED_SEARCH_RESULT_UNSUPPORTED".into())
    }
    pub fn accept_search_result(
        &mut self,
        ticket: &JobTicket,
        result: Arc<SearchResultV1>,
    ) -> Result<(), String> {
        if ticket.kind != JobKind::Search || !self.job_current(ticket) {
            self.reject_finished_job(ticket);
            return Err("STALE_SEARCH · результат не опубликован".into());
        }
        let checked = self
            .saved_search_domain
            .as_ref()
            .ok_or_else(|| "SEARCH_DOMAIN_NOT_SAVED".to_owned())
            .and_then(|saved| {
                validate_typed_search(&result, saved, &ticket.identity, &self.config)
            });
        if let Err(error) = checked {
            self.finish_error(ticket, &error);
            return Err(error);
        }
        self.status = format!(
            "SEARCH_DOMAIN_LIMITED · {:?} · {:?} · accepted={} diagnostic={}",
            result.status, result.conclusion, result.accepted, result.diagnostic_only
        );
        self.search_result = Some(result);
        self.candidate_source = Some(ticket.clone());
        self.selected_candidate = None;
        self.job = None;
        Ok(())
    }
    pub fn current_search_result(&self) -> Option<&SearchResultV1> {
        if !self.bound()
            || self.dirty
            || self.job.is_some()
            || !self.candidate_source.as_ref().is_some_and(|s| {
                self.identity
                    .as_ref()
                    .is_some_and(|i| i.same_source(&s.identity))
                    && s.search_generation == self.search_generation
            })
        {
            return None;
        }
        self.search_result.as_deref()
    }
    pub fn accept_report(
        &self,
        ui: &WidgetRef,
        cx: &mut Cx,
        ticket: &ReportTicket,
        report: &str,
    ) -> Result<(), String> {
        if self.current_result().is_none()
            || self
                .identity
                .as_ref()
                .is_none_or(|i| !i.same_source(&ticket.identity))
            || self.job.is_some()
            || self.search_generation != ticket.search_generation
            || self.candidate_source != ticket.candidate_source
        {
            return Err("STALE_REPORT · отчёт скрыт".into());
        }
        // Display is bounded; host retains the complete report for guarded export.
        ui.text_input(cx, ids!(gcc_report_text))
            .set_text(cx, &bounded(report, 64 * 1024));
        Ok(())
    }

    /// Only the small current editor identity is passed to the owner. Neither
    /// the display copy nor a caller-supplied archive authorizes persistence.
    pub fn snapshot_save_ticket(&self) -> Option<ReportTicket> {
        if self.current_result().is_none() || self.job.is_some() {
            return None;
        }
        Some(ReportTicket {
            identity: self.identity.as_ref()?.clone(),
            search_generation: self.search_generation,
            candidate_source: self.candidate_source.clone(),
        })
    }

    pub fn accept_snapshot_saved(
        &mut self,
        ticket: &ReportTicket,
        live: &DocumentBinding,
        archive_digest: &str,
    ) -> Result<(), String> {
        if self.binding.as_ref() != Some(live)
            || self.snapshot_save_ticket().as_ref().is_none_or(|current| !current.same_source(ticket))
        {
            return Err("SNAPSHOT_COMMITTED_ACK_STALE · обновите документ".into());
        }
        self.status = format!(
            "Расчёт сохранён в документе · rev {} · {} · требуется инженерное рассмотрение",
            live.revision, bounded(archive_digest, 96)
        );
        Ok(())
    }
    /// Existing section-space hit/drag controller calls this on a circle handle.
    /// This updates draft fields only and never commits or runs an analysis.
    pub fn drag_circle(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        expected: &DocumentBinding,
        generation: u64,
        center: Point2,
        radius: f64,
    ) -> Result<u64, String> {
        if !self.bound()
            || !self.case_config_loaded
            || self.binding.as_ref() != Some(expected)
            || self.draft_generation != generation
        {
            return Err("STALE_BINDING".into());
        }
        if !center.x.is_finite() || !center.z.is_finite() || !radius.is_finite() || radius <= 0.0 {
            return Err("Недопустимый предпросмотр окружности".into());
        }
        for (id, value) in [
            (id!(gcc_center_x), center.x),
            (id!(gcc_center_z), center.z),
            (id!(gcc_radius), radius),
        ] {
            ui.text_input(cx, &[id]).set_text(cx, &value.to_string());
        }
        ui.drop_down(cx, ids!(gcc_circle_mode))
            .set_selected_item(cx, 0);
        self.changed();
        self.refresh(ui, cx);
        Ok(self.draft_generation)
    }

    /// Disposable editor preview only; host validates it before constructing
    /// a render and marks incomplete/invalid pending geometry explicitly.
    pub fn pending_ground(&self) -> Option<(&GroundDefinition, u64)> {
        self.bound()
            .then(|| {
                self.draft_ground
                    .as_ref()
                    .map(|g| (g, self.draft_generation))
            })
            .flatten()
    }
    /// Host owns screen-to-section and camera/frame checks. Return generation
    /// becomes the next accepted sample's binding during a continuous drag.
    pub fn drag_point(
        &mut self,
        expected: &DocumentBinding,
        generation: u64,
        target: BoundaryTarget,
        index: usize,
        point: Point2,
    ) -> Result<u64, String> {
        if !self.bound()
            || self.binding.as_ref() != Some(expected)
            || self.draft_generation != generation
            || self.pending_rows.contains("point")
        {
            return Err("STALE_HANDLE_OR_UNAPPLIED_ROW".into());
        }
        if !point.x.is_finite() || !point.z.is_finite() {
            return Err("Координаты точки должны быть конечными".into());
        }
        let ground = self.draft_ground.as_mut().ok_or("Нет черновика Ground")?;
        let points = match target {
            BoundaryTarget::Terrain => &mut ground.terrain_polyline,
            BoundaryTarget::Bottom => &mut ground.bottom_boundary,
            BoundaryTarget::Interface(id) => {
                &mut ground
                    .layer_interfaces
                    .iter_mut()
                    .find(|v| v.interface_id.0 == id)
                    .ok_or("Граница удалена")?
                    .polyline
            }
        };
        *points.get_mut(index).ok_or("Точка удалена")? = point;
        self.ground_changed();
        Ok(self.draft_generation)
    }
    /// UIA SetValue and physical TextInputAction share this invalidation path.
    pub fn input_value_changed(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        id: LiveId,
    ) -> Result<(), String> {
        let (alias, _) = CIRCULAR_INPUTS
            .iter()
            .find(|(_, field)| *field == id)
            .ok_or("Неизвестный editable control ID")?;
        if id == id!(gcc_preview_width) {
            self.preview_generation = self.preview_generation.saturating_add(1);
            self.preview_width = None;
            let value = ui.text_input(cx, &[id]).text();
            if !value.trim().is_empty() {
                let width = parse_number(&value, "Ширина предпросмотра")?;
                if width <= 0.0 {
                    return Err("Ширина предпросмотра должна быть положительной".into());
                }
                self.preview_width = Some(width);
            }
        } else if alias.starts_with("geotech.circular.search.") {
            self.search_settings_changed();
        } else if ![
            id!(gcc_point_index),
            id!(gcc_region_index),
            id!(gcc_slice_id),
        ]
        .contains(&id)
        {
            if ground_input(id) {
                self.ground_changed();
            } else {
                self.changed();
            }
            if [id!(gcc_point_x), id!(gcc_point_z)].contains(&id) {
                self.pending_rows.insert("point");
            }
            if [
                id!(gcc_material_id),
                id!(gcc_material_name),
                id!(gcc_gamma),
                id!(gcc_cohesion),
                id!(gcc_phi),
                id!(gcc_provenance),
            ]
            .contains(&id)
            {
                self.pending_rows.insert("material");
            }
            if [id!(gcc_region_id), id!(gcc_region_material)].contains(&id) {
                self.pending_rows.insert("region");
            }
            if id == id!(gcc_interface_id) {
                self.pending_rows.insert("interface");
            }
        }
        Ok(())
    }
    fn search_settings_changed(&mut self) {
        self.archive_error = None;
        self.search_gate_accepted = false;
        self.search_dirty = true;
        self.update_dirty();
        self.search_generation = self.search_generation.saturating_add(1);
        self.candidate_source = None;
        self.selected_candidate = None;
        self.search_result = None;
        self.status = "SEARCH DRAFT DIRTY · заморозка даст локальный предпросмотр; сохранение в case выполняется отдельно".into();
    }
    pub fn observation(&self) -> serde_json::Value {
        let result = self.current_result();
        serde_json::json!({"active":self.active,"dirty":self.dirty,"stale":self.stale,
            "ground_dirty":self.ground_dirty,"case_dirty":self.case_dirty,"search_dirty":self.search_dirty,
            "ground_id":self.saved_ground.as_ref().map(|g|g.ground_section_id.to_string()),
            "case_id":self.saved_case.as_ref().map(|c|c.analysis_id.to_string()),
            "ground_source_operation_id":self.saved_ground.as_ref().map(|g|g.source_operation_id.get().to_string()),
            "case_source_operation_id":self.saved_case.as_ref().map(|c|c.source_operation_id.get().to_string()),
            "analysis_host_gate":self.analysis_gate_accepted,"search_host_gate":self.search_gate_accepted,
            "draft_generation":self.draft_generation,"search_generation":self.search_generation,
            "status":self.status,"view_dimension":if self.view_3d{"3D_extrusion"}else{"2D"},
            "presentation_state":self.presentation_state(),
            "archive":self.archived_view().map(|view|&view.presentation),
            "archive_error":self.archive_error,
            "display_result_origin":if self.current_result().is_some() || self.current_search_result().is_some(){"current_calculated"}else if self.archived_view().is_some(){"document_archive"}else{"none"},
            "display_slice_count":self.display_result().map(|result|result.slices.len()),
            "display_fellenius_fs":self.display_result().map(|result|result.fellenius_result.factor_of_safety),
            "display_bishop_equation_fs":self.display_result().map(|result|result.bishop_result.factor_of_safety),
            "display_bishop_accepted_fs":self.display_result().map(|result|result.bishop_result.accepted_factor_of_safety),
            "calculation_dimension":"2D","result_current":result.is_some(),
            "core_accepted":result.map(|r|r.accepted),"diagnostic_only":result.map(|r|r.diagnostic_only),
            "input_fingerprint":result.map(|r|&r.input_fingerprint),"source_fingerprint":result.map(|r|&r.source_fingerprint),
            "solver_version":result.map(|r|&r.solver_version),"slice_page":self.page,"slice_page_size":PAGE_SIZE,
            "job_sequence":self.job.as_ref().map(|j|j.sequence),"cancelling":self.cancelling,
            "geometry_endpoint_review":format!("{:?}",self.review.geometry_endpoints.physical_status),
            "finite_table_review":format!("{:?}",self.review.finite_table.physical_status),
            "whole_arc_review":format!("{:?}",self.review.whole_arc_status()),
            "whole_arc_applicability":self.review.whole_arc_applicability,
            "search_classification":"SEARCH_DOMAIN_LIMITED",
            "saved_search_domain":self.saved_search_domain.is_some(),
            "typed_search_status":self.current_search_result().map(|r|format!("{:?}",r.status)),
            "typed_search_accepted":self.current_search_result().map(search_is_accepted),
            "search_request_fingerprint":self.current_search_result().map(|r|&r.request_fingerprint),
            "render_result_eligible":self.result_for_overlays().is_some() || self.archived_for_overlays().is_some(),
            "requested_overlays":{"ground":self.flags.ground,"soil_layers":self.flags.soil_layers,
                "failure_mass":self.flags.failure_mass,"slices":self.flags.slices,"forces":self.flags.forces,"selected_circle":self.flags.critical_circle}})
    }
    pub fn handle_actions(
        &mut self,
        ui: &WidgetRef,
        cx: &mut Cx,
        actions: &Actions,
    ) -> Vec<CircularUiIntent> {
        if !self.active {
            return vec![];
        }
        let mut intents = vec![];
        let before_draft = self.draft_generation;
        let before_search = self.search_generation;
        let before_preview = self.preview_generation;
        for (_, id) in CIRCULAR_INPUTS {
            if ui.text_input(cx, &[*id]).changed(actions).is_some() {
                if let Err(error) = self.input_value_changed(ui, cx, *id) {
                    self.status = error;
                }
            }
        }
        if ui
            .drop_down(cx, ids!(gcc_search_target))
            .selected(actions)
            .is_some()
            || ui
                .check_box(cx, ids!(gcc_search_enabled))
                .changed(actions)
                .is_some()
        {
            self.search_settings_changed();
        }
        if ui
            .drop_down(cx, ids!(gcc_direction))
            .selected(actions)
            .is_some()
            || ui
                .drop_down(cx, ids!(gcc_circle_mode))
                .selected(actions)
                .is_some()
            || ui
                .check_box(cx, ids!(gcc_refine))
                .changed(actions)
                .is_some()
        {
            self.changed();
        }
        for (id, is_case) in [
            (id!(gcc_ground_selector), false),
            (id!(gcc_case_selector), true),
        ] {
            if let Some(index) = ui.drop_down(cx, &[id]).selected(actions) {
                self.load_generation = self.load_generation.saturating_add(1);
                self.clear_archive_view();
                if is_case {
                    self.case_pick = index
                        .checked_sub(1)
                        .and_then(|i| self.cases.get(i))
                        .map(|c| c.analysis_id);
                } else {
                    self.ground_pick = index
                        .checked_sub(1)
                        .and_then(|i| self.grounds.get(i))
                        .map(|g| g.ground_section_id);
                }
            }
        }
        if let Some(index) = ui.drop_down(cx, ids!(gcc_case_ground)).selected(actions) {
            let selected = index
                .checked_sub(1)
                .and_then(|i| self.grounds.get(i))
                .map(|g| g.ground_section_id);
            if selected != self.case_ground_ref {
                self.case_ground_ref = selected;
                self.changed();
            }
        }
        if let Some(index) = ui
            .drop_down(cx, ids!(gcc_material_selector))
            .selected(actions)
        {
            if self.pending_rows.contains("material") {
                ui.drop_down(cx, ids!(gcc_material_selector))
                    .set_selected_item(cx, self.material_selection);
                self.status = "UNAPPLIED_ROW · примените материал или явно перечитайте всю постановку; текст сохранён".into();
            } else if let Some(m) = self
                .draft_ground
                .as_ref()
                .and_then(|g| index.checked_sub(1).and_then(|i| g.materials.get(i)))
            {
                for (id, value) in [
                    (id!(gcc_material_id), m.material_id.0.clone()),
                    (id!(gcc_material_name), m.properties.name.clone()),
                    (
                        id!(gcc_gamma),
                        m.properties.unit_weight_kn_per_m3.to_string(),
                    ),
                    (
                        id!(gcc_cohesion),
                        m.properties.effective_cohesion_kpa.to_string(),
                    ),
                    (
                        id!(gcc_phi),
                        m.properties.effective_friction_angle_deg.to_string(),
                    ),
                    (id!(gcc_provenance), m.properties.provenance.clone()),
                ] {
                    ui.text_input(cx, &[id]).set_text(cx, &value);
                }
                self.pending_rows.remove("material");
                self.material_selection = index;
            } else if index == 0 {
                for id in [
                    id!(gcc_material_id),
                    id!(gcc_material_name),
                    id!(gcc_gamma),
                    id!(gcc_cohesion),
                    id!(gcc_phi),
                    id!(gcc_provenance),
                ] {
                    ui.text_input(cx, &[id]).set_text(cx, "");
                }
                self.material_selection = 0;
            }
        }
        for (id, old) in [
            (id!(gcc_point_boundary), self.point_boundary),
            (id!(gcc_interface_selector), self.interface_selection),
        ] {
            if let Some(index) = ui.drop_down(cx, &[id]).selected(actions) {
                if self.pending_rows.contains("point") {
                    ui.drop_down(cx, &[id]).set_selected_item(cx, old);
                    self.status =
                        "UNAPPLIED_ROW · сначала примените координаты точки; текст сохранён".into();
                } else {
                    if id == id!(gcc_point_boundary) {
                        self.point_boundary = index;
                    } else {
                        self.interface_selection = index;
                    }
                    for field in [id!(gcc_point_x), id!(gcc_point_z)] {
                        ui.text_input(cx, &[field]).set_text(cx, "");
                    }
                }
            }
        }
        for (id, field) in [
            (id!(gcc_ground), &mut self.flags.ground),
            (id!(gcc_layers), &mut self.flags.soil_layers),
            (id!(gcc_mass), &mut self.flags.failure_mass),
            (id!(gcc_slices), &mut self.flags.slices),
            (id!(gcc_forces), &mut self.flags.forces),
            (id!(gcc_circle), &mut self.flags.critical_circle),
        ] {
            if let Some(value) = ui.check_box(cx, &[id]).changed(actions) {
                *field = value;
                intents.push(CircularUiIntent::ViewChanged);
            }
        }
        if ui.button(cx, ids!(gcc_new)).clicked(actions) {
            match self
                .live_binding
                .clone()
                .ok_or("Нет Document/session".into())
                .and_then(|live| self.new_ground_draft(ui, cx, live))
            {
                Ok(()) => intents.push(CircularUiIntent::ViewChanged),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_reload)).clicked(actions) {
            match self
                .ground_pick
                .ok_or("Выберите сохранённый разрез".into())
                .and_then(|id| self.record_load_ticket(RecordLoadMode::Ground, id, None))
            {
                Ok(ticket) => intents.push(CircularUiIntent::LoadGround(ticket)),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_case_new)).clicked(actions) {
            if !self.ground_dirty && self.job.is_none() {
                match self
                    .case_ground_ref
                    .ok_or("Выберите существующий разрез для нового case".into())
                    .and_then(|id| self.record_load_ticket(RecordLoadMode::NewCase, id, None))
                {
                    Ok(ticket) => intents.push(CircularUiIntent::NewCase(ticket)),
                    Err(error) => self.status = error,
                }
            } else {
                self.status = "Сначала сохраните разрез и завершите задачу".into();
            }
        }
        if ui.button(cx, ids!(gcc_case_reload)).clicked(actions) {
            match self
                .case_pick
                .and_then(|id| self.cases.iter().find(|c| c.analysis_id == id))
                .map(|case| (case.definition.ground_section_id, case.analysis_id))
                .ok_or("Выберите сохранённый case".into())
                .and_then(|(ground_id, analysis_id)| {
                    self.record_load_ticket(RecordLoadMode::Case, ground_id, Some(analysis_id))
                }) {
                Ok(ticket) => intents.push(CircularUiIntent::LoadCase(ticket)),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_config_edit)).clicked(actions) {
            match self.edited_config(ui, cx) {
                Ok(config) if self.bound() && self.case_config_loaded => {
                    match self.editor_context() {
                        Ok(context) => {
                            intents.push(CircularUiIntent::EditFullConfig { context, config })
                        }
                        Err(error) => self.status = error,
                    }
                }
                Ok(_) => self.status = "Сначала создайте/загрузите case с полным config".into(),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_search_freeze)).clicked(actions) {
            if let Err(error) = self.freeze_search_settings(ui, cx) {
                self.status = error;
            }
        }
        if ui.button(cx, ids!(gcc_search_policy)).clicked(actions) {
            if !self.ready_for_analysis()
                || self.job.is_some()
                || self.ground_dirty
                || self.case_dirty
                || !self.pending_rows.is_empty()
            {
                self.status = "Сначала сохраните правки и разрешите актуальный ground/case".into();
            } else {
                match self.editor_context() {
                    Ok(context) => intents.push(CircularUiIntent::EditSearchPolicy {
                        context,
                        request: self.search_request_template.clone(),
                    }),
                    Err(error) => self.status = error,
                }
            }
        }
        if ui.button(cx, ids!(gcc_search_save)).clicked(actions) {
            match self.save_search_proposal(ui, cx) {
                Ok(proposal) => intents.push(CircularUiIntent::SaveCase(proposal)),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_cancel)).clicked(actions) {
            if let Some(job) = self.job.clone() {
                self.cancelling = true;
                self.status = "CANCELLING · ожидается подтверждение worker".into();
                intents.push(CircularUiIntent::Cancel(job));
            }
        }
        for id in [
            id!(gcc_point_load),
            id!(gcc_point_add),
            id!(gcc_point_apply),
            id!(gcc_point_remove),
            id!(gcc_interface_add),
            id!(gcc_interface_remove),
            id!(gcc_material_apply),
            id!(gcc_material_remove),
            id!(gcc_region_load),
            id!(gcc_region_apply),
            id!(gcc_region_remove),
        ] {
            if ui.button(cx, &[id]).clicked(actions) {
                if let Err(error) = self.edit_row(ui, cx, id) {
                    self.status = error;
                }
            }
        }
        if ui.button(cx, ids!(gcc_circle_preview)).clicked(actions) {
            match if self.bound()
                && self.case_config_loaded
                && !self.ground_dirty
                && self.saved_ground.as_ref().is_some_and(|g| {
                    inspect_ground_readiness(&g.definition).is_ok_and(|r| r.is_ready())
                }) {
                self.circle(ui, cx, &self.config)
            } else {
                Err("STALE_BINDING".into())
            } {
                Ok(circle) => intents.push(CircularUiIntent::PreviewCircle(circle)),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_ground_save)).clicked(actions) {
            match self.save_ground_proposal(ui, cx) {
                Ok(proposal) => intents.push(CircularUiIntent::SaveGround(proposal)),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_case_save)).clicked(actions) {
            match self.save_case_proposal(ui, cx) {
                Ok(proposal) => intents.push(CircularUiIntent::SaveCase(proposal)),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_analyze)).clicked(actions) {
            match self.begin_job(JobKind::Analyze) {
                Ok(ticket) => intents.push(CircularUiIntent::Analyze(ticket)),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_search)).clicked(actions) {
            match self
                .saved_search_domain
                .clone()
                .ok_or("SEARCH_DOMAIN_NOT_SAVED".into())
                .and_then(|spec| self.begin_job(JobKind::Search).map(|job| (job, spec)))
            {
                Ok((job, spec)) => intents.push(CircularUiIntent::Search(job, spec)),
                Err(error) => self.status = error,
            }
        }
        if ui.button(cx, ids!(gcc_2d)).clicked(actions) {
            self.view_3d = false;
            intents.push(CircularUiIntent::ViewChanged);
        }
        if ui.button(cx, ids!(gcc_3d)).clicked(actions) {
            self.view_3d = true;
            intents.push(CircularUiIntent::ViewChanged);
        }
        if ui.button(cx, ids!(gcc_fit)).clicked(actions) {
            intents.push(CircularUiIntent::Fit);
        }
        if ui.button(cx, ids!(gcc_slices_prev)).clicked(actions) {
            self.page = self.page.saturating_sub(1);
        }
        if ui.button(cx, ids!(gcc_slices_next)).clicked(actions) {
            let pages = self
                .display_result()
                .map_or(0, |r| r.slices.len().div_ceil(PAGE_SIZE));
            if self.page + 1 < pages {
                self.page += 1;
            }
        }
        if ui.button(cx, ids!(gcc_slice_select)).clicked(actions) {
            match integer(ui, cx, id!(gcc_slice_id), "ID отсека") {
                Ok(id)
                    if self
                        .display_result()
                        .is_some_and(|r| r.slices.iter().any(|s| s.slice_id == id)) =>
                {
                    intents.push(CircularUiIntent::SelectSlice(id))
                }
                _ => self.status = "Нет такого отсека в текущем результате".into(),
            }
        }
        for (id, apply) in [
            (id!(gcc_candidate_select), false),
            (id!(gcc_candidate_apply), true),
        ] {
            if ui.button(cx, &[id]).clicked(actions) {
                let index = ui
                    .drop_down(cx, ids!(gcc_candidate_selector))
                    .selected_item();
                if self.bound() && !self.dirty && self.job.is_none() {
                    if let (Some(result), Some(search)) =
                        (self.current_search_result(), self.candidate_source.as_ref())
                    {
                        if let Some(row) = result.candidates.get(index) {
                            if apply {
                                if candidate_can_apply(result, row) {
                                    intents.push(CircularUiIntent::ApplyCandidate {
                                        expected: self.binding.clone().unwrap(),
                                        search: search.clone(),
                                        id: row.id,
                                    });
                                } else {
                                    self.status = "Кандидат доступен только как диагностика; accepted critical surface отсутствует".into();
                                }
                            } else {
                                intents.push(CircularUiIntent::SelectCandidate {
                                    search: search.clone(),
                                    id: row.id,
                                });
                            }
                        }
                    }
                }
            }
        }
        if ui.button(cx, ids!(gcc_report)).clicked(actions)
            && self.current_result().is_some()
            && self.job.is_none()
        {
            intents.push(CircularUiIntent::Report(ReportTicket {
                identity: self.identity.clone().unwrap(),
                search_generation: self.search_generation,
                candidate_source: self.candidate_source.clone(),
            }));
        }
        if ui.button(cx, ids!(gcc_snapshot_save)).clicked(actions) {
            if let Some(ticket) = self.snapshot_save_ticket() {
                intents.push(CircularUiIntent::SaveSnapshot(ticket));
            }
        }
        if before_draft != self.draft_generation
            || before_search != self.search_generation
            || before_preview != self.preview_generation
        {
            intents.push(CircularUiIntent::ViewChanged);
        }
        self.refresh(ui, cx);
        intents
    }

    fn points_mut<'a>(
        ground: &'a mut GroundDefinition,
        boundary: usize,
        interface: usize,
    ) -> Result<&'a mut Vec<Point2>, String> {
        match boundary {
            0 => Ok(&mut ground.terrain_polyline),
            1 => Ok(&mut ground.bottom_boundary),
            2 => ground
                .layer_interfaces
                .get_mut(interface)
                .map(|v| &mut v.polyline)
                .ok_or("Сначала создайте и выберите границу".into()),
            _ => Err("Неизвестная граница".into()),
        }
    }
    fn edit_row(&mut self, ui: &WidgetRef, cx: &mut Cx, id: LiveId) -> Result<(), String> {
        if !self.bound() {
            return Err("STALE_BINDING · черновик сохранён; сначала перечитайте Document".into());
        }
        if (id == id!(gcc_point_load) && self.pending_rows.contains("point"))
            || (id == id!(gcc_region_load) && self.pending_rows.contains("region"))
        {
            return Err("UNAPPLIED_ROW · примените введённые значения или явно перечитайте всю постановку; текст сохранён".into());
        }
        let limits = ResourceLimits::default(); // owner standalone-ground limits, independent of case config
        let boundary = ui.drop_down(cx, ids!(gcc_point_boundary)).selected_item();
        let interface = ui
            .drop_down(cx, ids!(gcc_interface_selector))
            .selected_item();
        let ground = self
            .draft_ground
            .as_mut()
            .ok_or("Создайте или загрузите Ground Model")?;
        let point_count = ground
            .terrain_polyline
            .len()
            .checked_add(ground.bottom_boundary.len())
            .and_then(|n| {
                ground
                    .layer_interfaces
                    .iter()
                    .try_fold(n, |n, i| n.checked_add(i.polyline.len()))
            })
            .ok_or("Лимит точек: переполнение")?;
        let mut mutated = true;
        if [
            id!(gcc_point_load),
            id!(gcc_point_add),
            id!(gcc_point_apply),
            id!(gcc_point_remove),
        ]
        .contains(&id)
        {
            let index = integer(ui, cx, id!(gcc_point_index), "Индекс точки")?;
            let points = Self::points_mut(ground, boundary, interface)?;
            if id == id!(gcc_point_load) {
                let p = points.get(index).ok_or("Нет точки с указанным индексом")?;
                ui.text_input(cx, ids!(gcc_point_x))
                    .set_text(cx, &p.x.to_string());
                ui.text_input(cx, ids!(gcc_point_z))
                    .set_text(cx, &p.z.to_string());
                mutated = false;
            } else if id == id!(gcc_point_remove) {
                if index >= points.len() {
                    return Err("Индекс удаляемой точки вне границ".into());
                }
                points.remove(index);
            } else {
                let point = Point2::new(
                    number(ui, cx, id!(gcc_point_x), "X точки")?,
                    number(ui, cx, id!(gcc_point_z), "Z точки")?,
                );
                if id == id!(gcc_point_add) {
                    if point_count >= limits.max_boundary_points.min(32768)
                        || (boundary == 0 && points.len() > limits.max_terrain_segments)
                        || index > points.len()
                    {
                        return Err("Индекс вставки или лимит точек недопустим".into());
                    }
                    points.insert(index, point);
                } else {
                    *points.get_mut(index).ok_or("Нет точки для изменения")? = point;
                }
            }
            self.pending_rows.remove("point");
        } else if id == id!(gcc_interface_add) {
            let name = text(ui, cx, id!(gcc_interface_id), MAX_ID)?;
            if ground.layer_interfaces.len() + 1 >= limits.max_soil_regions
                || ground
                    .layer_interfaces
                    .iter()
                    .any(|i| i.interface_id.0 == name)
            {
                return Err("Повтор ID границы или превышен лимит слоёв".into());
            }
            ground.layer_interfaces.push(LayerInterface {
                interface_id: LayerInterfaceId(name),
                polyline: vec![],
            });
            self.pending_rows.remove("interface");
        } else if id == id!(gcc_interface_remove) {
            if interface >= ground.layer_interfaces.len() {
                return Err("Нет выбранной границы".into());
            }
            ground.layer_interfaces.remove(interface);
        } else if id == id!(gcc_material_apply) {
            let material = GroundMaterial {
                material_id: SoilMaterialId(text(ui, cx, id!(gcc_material_id), MAX_ID)?),
                properties: SoilMaterial {
                    name: text(ui, cx, id!(gcc_material_name), MAX_ID)?,
                    unit_weight_kn_per_m3: positive(ui, cx, id!(gcc_gamma), "γ, кН/м³")?,
                    effective_cohesion_kpa: number(ui, cx, id!(gcc_cohesion), "c′, кПа")?,
                    effective_friction_angle_deg: number(ui, cx, id!(gcc_phi), "φ′, градусы")?,
                    provenance: text(ui, cx, id!(gcc_provenance), MAX_TEXT)?,
                },
            };
            if material.properties.effective_cohesion_kpa < 0.0
                || !(0.0..90.0).contains(&material.properties.effective_friction_angle_deg)
            {
                return Err("Требуется c′ ≥ 0 и 0 ≤ φ′ < 90°".into());
            }
            if let Some(old) = ground
                .materials
                .iter_mut()
                .find(|m| m.material_id == material.material_id)
            {
                *old = material;
            } else {
                if ground.materials.len() >= limits.max_soil_regions {
                    return Err("Лимит материалов достигнут".into());
                }
                ground.materials.push(material);
            }
            self.pending_rows.remove("material");
        } else if id == id!(gcc_material_remove) {
            let material = text(ui, cx, id!(gcc_material_id), MAX_ID)?;
            if ground
                .soil_regions
                .iter()
                .any(|r| r.material_id.0 == material)
            {
                return Err("Материал назначен области; сначала явно измените назначение".into());
            }
            let old = ground.materials.len();
            ground.materials.retain(|m| m.material_id.0 != material);
            if old == ground.materials.len() {
                return Err("Не найден Material ID".into());
            }
            self.pending_rows.remove("material");
        } else {
            let index = integer(ui, cx, id!(gcc_region_index), "Индекс области")?;
            if id == id!(gcc_region_load) {
                let row = ground
                    .soil_regions
                    .get(index)
                    .ok_or("Нет области с указанным индексом")?;
                ui.text_input(cx, ids!(gcc_region_id))
                    .set_text(cx, &row.region_id.0);
                ui.text_input(cx, ids!(gcc_region_material))
                    .set_text(cx, &row.material_id.0);
                mutated = false;
            } else if id == id!(gcc_region_remove) {
                if index >= ground.soil_regions.len() {
                    return Err("Индекс удаляемой области вне границ".into());
                }
                ground.soil_regions.remove(index);
            } else {
                let row = SoilRegion {
                    region_id: SoilRegionId(text(ui, cx, id!(gcc_region_id), MAX_ID)?),
                    material_id: SoilMaterialId(text(ui, cx, id!(gcc_region_material), MAX_ID)?),
                };
                if !ground
                    .materials
                    .iter()
                    .any(|m| m.material_id == row.material_id)
                {
                    return Err("Указанный Material ID не существует".into());
                }
                if ground
                    .soil_regions
                    .iter()
                    .enumerate()
                    .any(|(i, r)| i != index && r.region_id == row.region_id)
                {
                    return Err("Region ID уже назначен другой области".into());
                }
                if index == ground.soil_regions.len() && index < limits.max_soil_regions {
                    ground.soil_regions.push(row);
                } else {
                    *ground
                        .soil_regions
                        .get_mut(index)
                        .ok_or("Индекс области вне границ")? = row;
                }
            }
            self.pending_rows.remove("region");
        }
        if mutated {
            self.ground_changed();
        }
        Ok(())
    }

    pub fn refresh(&self, ui: &WidgetRef, cx: &mut Cx) {
        set_labels(
            ui,
            cx,
            id!(gcc_ground_selector),
            std::iter::once("Выберите сохранённый разрез".into())
                .chain(
                    self.grounds
                        .iter()
                        .map(|g| format!("{} · {}", g.ground_section_id, g.name)),
                )
                .collect(),
        );
        set_labels(
            ui,
            cx,
            id!(gcc_case_ground),
            std::iter::once("Ссылка на существующий разрез".into())
                .chain(
                    self.grounds
                        .iter()
                        .map(|g| format!("{} · {}", g.ground_section_id, g.name)),
                )
                .collect(),
        );
        set_labels(
            ui,
            cx,
            id!(gcc_case_selector),
            std::iter::once("Выберите сохранённый case".into())
                .chain(self.cases.iter().map(|c| {
                    format!(
                        "{} · {} · ground {}",
                        c.analysis_id, c.name, c.definition.ground_section_id
                    )
                }))
                .collect(),
        );
        ui.drop_down(cx, ids!(gcc_ground_selector))
            .set_selected_item(
                cx,
                self.grounds
                    .iter()
                    .position(|g| Some(g.ground_section_id) == self.ground_pick)
                    .map_or(0, |i| i + 1),
            );
        ui.drop_down(cx, ids!(gcc_case_ground)).set_selected_item(
            cx,
            self.grounds
                .iter()
                .position(|g| Some(g.ground_section_id) == self.case_ground_ref)
                .map_or(0, |i| i + 1),
        );
        ui.drop_down(cx, ids!(gcc_case_selector)).set_selected_item(
            cx,
            self.cases
                .iter()
                .position(|c| Some(c.analysis_id) == self.case_pick)
                .map_or(0, |i| i + 1),
        );
        ui.label(cx, ids!(gcc_ground_id)).set_text(
            cx,
            &self.saved_ground.as_ref().map_or(
                "Новый разрез · ID назначает Document".into(),
                |g| format!("Ground ID: {}", g.ground_section_id),
            ),
        );
        ui.label(cx, ids!(gcc_analysis_id)).set_text(
            cx,
            &self.saved_case.as_ref().map_or(
                "Новый case · ID назначает Document".into(),
                |c| format!("Case ID: {}", c.analysis_id),
            ),
        );
        if !self.ground_dirty {
            if let Some(g) = &self.saved_ground {
                ui.text_input(cx, ids!(gcc_ground_name))
                    .set_text(cx, &g.name);
            }
        }
        if !self.case_dirty {
            if let Some(c) = &self.saved_case {
                ui.text_input(cx, ids!(gcc_case_name)).set_text(cx, &c.name);
            }
        }
        let readiness = self
            .draft_ground
            .as_ref()
            .map_or(
                "Нет разреза".into(),
                |g| match inspect_ground_readiness(g) {
                    Ok(r) => format!(
                        "{:?} · missing {:?}{}",
                        r.state,
                        r.missing,
                        if self.ground_dirty {
                            " · не сохранён"
                        } else {
                            ""
                        }
                    ),
                    Err(e) => format!("Ошибка структуры разреза: {e}"),
                },
            );
        ui.label(cx, ids!(gcc_readiness))
            .set_text(cx, &bounded(&readiness, 4096));
        ui.label(cx, ids!(gcc_case_sources)).set_text(
            cx,
            &format!(
                "Ground source: {} · case source: {}",
                self.saved_ground
                    .as_ref()
                    .map_or("нет".into(), |g| g.source_operation_id.get().to_string()),
                self.saved_case
                    .as_ref()
                    .map_or("нет".into(), |c| c.source_operation_id.get().to_string())
            ),
        );
        let full_config = if self.case_config_loaded {
            match self.edited_config(ui, cx) {
                Ok(config) => format!(
                    "{config:#?}\n{}",
                    if self.case_dirty {
                        "Изменения case ещё не сохранены"
                    } else {
                        "Config сохранённого case"
                    }
                ),
                Err(error) => format!(
                    "Ошибка ввода config: {error}\nПоследний загруженный полный config:\n{:#?}",
                    self.config
                ),
            }
        } else {
            "Полный config case не загружен; скрытые значения не используются".into()
        };
        ui.text_input(cx, ids!(gcc_full_config))
            .set_text(cx, &bounded(&full_config, 16 * 1024));
        ui.label(cx, ids!(gcc_search_config)).set_text(
            cx,
            &if self.case_config_loaded {
                search_policy_text(
                    self.search_request_template.as_ref(),
                    &self.config,
                    self.saved_case
                        .as_ref()
                        .is_some_and(|case| case.definition.search_spec.is_some()),
                    self.resolved_search.is_some(),
                )
            } else {
                "Полный config case не загружен".into()
            },
        );
        if self.job.is_some() {
            ui.text_input(cx, ids!(gcc_report_text))
                .set_text(cx, "RUNNING · прежний отчёт скрыт");
        }
        ui.label(cx, ids!(gcc_status)).set_text(cx, &format!("{} · {}", self.presentation_state(), self.status));
        let archive = self.archived_view();
        ui.label(cx, ids!(gcc_archive_status)).set_text(cx, &self.archive_banner());
        ui.text_input(cx, ids!(gcc_archive_provenance)).set_text(cx, &self.archive_provenance_text());
        ui.label(cx, ids!(gcc_binding)).set_text(
            cx,
            &self
                .binding
                .as_ref()
                .map_or("Нет Document binding".into(), |b| {
                    format!(
                        "{} · session {} · open {} · rev {}{}",
                        b.document_uuid,
                        b.session_id,
                        b.open_generation,
                        b.revision,
                        if self.stale { " · STALE" } else { "" }
                    )
                }),
        );
        let idle = self.job.is_none();
        let ready = self.ready_for_analysis() && idle;
        ui.button(cx, ids!(gcc_analyze))
            .set_enabled(cx, ready && self.analysis_gate_accepted);
        ui.button(cx, ids!(gcc_search)).set_enabled(
            cx,
            ready && self.search_gate_accepted && self.search_dispatch_ready(),
        );
        ui.button(cx, ids!(gcc_search_freeze))
            .set_enabled(cx, ready);
        let can_edit_search =
            ready && !self.ground_dirty && !self.case_dirty && self.pending_rows.is_empty();
        ui.button(cx, ids!(gcc_search_policy))
            .set_enabled(cx, can_edit_search);
        let can_save_search = self.save_search_proposal(ui, cx).is_ok();
        ui.button(cx, ids!(gcc_search_save))
            .set_enabled(cx, can_save_search);
        ui.button(cx, ids!(gcc_cancel))
            .set_enabled(cx, !idle && !self.cancelling);
        ui.button(cx, ids!(gcc_ground_save))
            .set_enabled(cx, self.bound() && idle && self.draft_ground.is_some());
        ui.button(cx, ids!(gcc_case_save)).set_enabled(
            cx,
            self.bound()
                && idle
                && !self.ground_dirty
                && self.case_config_loaded
                && self.case_ground_ref.is_some(),
        );
        ui.button(cx, ids!(gcc_new))
            .set_enabled(cx, idle && self.live_binding.is_some());
        ui.button(cx, ids!(gcc_reload))
            .set_enabled(cx, idle && self.ground_pick.is_some());
        ui.button(cx, ids!(gcc_case_new)).set_enabled(
            cx,
            self.bound() && idle && !self.ground_dirty && self.case_ground_ref.is_some(),
        );
        ui.button(cx, ids!(gcc_case_reload))
            .set_enabled(cx, idle && self.case_pick.is_some());
        ui.button(cx, ids!(gcc_config_edit))
            .set_enabled(cx, self.bound() && idle && self.case_config_loaded);
        ui.button(cx, ids!(gcc_circle_preview)).set_enabled(
            cx,
            self.bound()
                && self.case_config_loaded
                && !self.ground_dirty
                && self.saved_ground.as_ref().is_some_and(|g| {
                    inspect_ground_readiness(&g.definition).is_ok_and(|r| r.is_ready())
                }),
        );
        ui.button(cx, ids!(gcc_report))
            .set_enabled(cx, self.current_result().is_some() && idle);
        // The App owner enables this only after checking the owned report/job.
        ui.button(cx, ids!(gcc_snapshot_save)).set_enabled(cx, false);
        if let Some(ground) = &self.draft_ground {
            let labels = std::iter::once("Новый материал".into())
                .chain(
                    ground
                        .materials
                        .iter()
                        .map(|m| format!("{} · {}", m.material_id.0, m.properties.name)),
                )
                .collect();
            set_labels(ui, cx, id!(gcc_material_selector), labels);
            set_labels(
                ui,
                cx,
                id!(gcc_interface_selector),
                if ground.layer_interfaces.is_empty() {
                    vec!["Нет границ".into()]
                } else {
                    ground
                        .layer_interfaces
                        .iter()
                        .map(|i| i.interface_id.0.clone())
                        .collect()
                },
            );
            let mut rows = String::new();
            for (name, line) in std::iter::once(("terrain", ground.terrain_polyline.as_slice()))
                .chain(
                    ground
                        .layer_interfaces
                        .iter()
                        .map(|i| (i.interface_id.0.as_str(), i.polyline.as_slice())),
                )
                .chain(std::iter::once((
                    "bottom",
                    ground.bottom_boundary.as_slice(),
                )))
            {
                let _ = writeln!(rows, "{name}:");
                for (i, p) in line.iter().enumerate() {
                    let _ = writeln!(rows, "  {i}: X={} Z={}", p.x, p.z);
                }
            }
            ui.text_input(cx, ids!(gcc_geometry_rows))
                .set_text(cx, &bounded(&rows, 64 * 1024));
            let rows = ground
                .soil_regions
                .iter()
                .enumerate()
                .map(|(i, r)| format!("{i}: {} → {}", r.region_id.0, r.material_id.0))
                .collect::<Vec<_>>()
                .join("\n");
            ui.text_input(cx, ids!(gcc_region_rows)).set_text(cx, &rows);
        }
        if let Some(result) = self.display_result() {
            let review = if self.current_result().is_some() { &self.review }
                else { &self.archive.as_ref().expect("display archive").review };
            ui.text_input(cx, ids!(gcc_summary))
                .set_text(cx, &summary(result, review));
            ui.text_input(cx, ids!(gcc_slices_table))
                .set_text(cx, &slice_page(result, self.page));
            ui.label(cx, ids!(gcc_slices_page)).set_text(
                cx,
                &format!(
                    "Страница {} / {} · {} отсеков",
                    self.page + 1,
                    result.slices.len().div_ceil(PAGE_SIZE),
                    result.slices.len()
                ),
            );
        } else {
            ui.text_input(cx, ids!(gcc_summary)).set_text(
                cx,
                "Нет результата для выбранного case. Сохранённые значения появляются после проверки архива без запуска расчёта.",
            );
            ui.text_input(cx, ids!(gcc_slices_table))
                .set_text(cx, "Нет текущей таблицы отсеков");
            ui.label(cx, ids!(gcc_slices_page)).set_text(cx, "0 / 0");
            ui.text_input(cx, ids!(gcc_report_text))
                .set_text(cx, "Нет текущего отчёта");
        }
        if self.current_result().is_none() && self.current_search_result().is_none() {
            if let Some(saved) = archive.filter(|saved| saved.snapshot.is_some()) {
                ui.text_input(cx, ids!(gcc_report_text)).set_text(cx,
                    &format!("{}\nСохранённый отчёт · новый расчёт не запускался\n{}", self.archive_banner(), saved.report_preview));
            }
        }
        let search = self.display_search_result();
        let candidates = search.map_or(&[][..], |r| r.candidates.as_slice());
        ui.text_input(cx, ids!(gcc_search_minima)).set_text(
            cx,
            &search.map_or_else(
                || "SEARCH_DOMAIN_LIMITED · NOT_RUN / STALE".into(),
                minima_text,
            ),
        );
        ui.button(cx, ids!(gcc_candidate_select))
            .set_enabled(cx, self.current_search_result().is_some() && !candidates.is_empty());
        let selected = ui
            .drop_down(cx, ids!(gcc_candidate_selector))
            .selected_item();
        ui.button(cx, ids!(gcc_candidate_apply)).set_enabled(
            cx,
            self.current_search_result().is_some_and(|r| {
                candidates
                    .get(selected)
                    .is_some_and(|c| candidate_can_apply(r, c))
            }),
        );
        set_labels(
            ui,
            cx,
            id!(gcc_candidate_selector),
            if candidates.is_empty() {
                vec!["Нет текущих кандидатов".into()]
            } else {
                candidates.iter().map(|r| r.id.to_string()).collect()
            },
        );
        let rows = search.map_or_else(|| "Нет текущих кандидатов".into(), search_result_text);
        ui.text_input(cx, ids!(gcc_candidates_table))
            .set_text(cx, &bounded(&rows, 128 * 1024));
        ui.label(cx, ids!(gcc_search_status)).set_text(
            cx,
            &if self.cancelling {
                "Отмена запрошена; ожидается подтверждение".into()
            } else if self.job.as_ref().is_some_and(|j| j.kind == JobKind::Search) {
                "Поиск выполняется; отмена доступна".into()
            } else if let Some(result) = search {
                format!(
                    "SEARCH_DOMAIN_LIMITED · {:?} · accepted={} · diagnostic={}",
                    result.status, result.accepted, result.diagnostic_only
                )
            } else if self.search_gate_accepted {
                "NOT_RUN".into()
            } else {
                "SEARCH_GATE_NOT_ACCEPTED".into()
            },
        );
    }
}

fn ground_input(id: LiveId) -> bool {
    CIRCULAR_INPUTS
        .iter()
        .find(|(_, v)| *v == id)
        .is_some_and(|(a, _)| {
            *a == "geotech.circular.ground.name"
                || a.starts_with("geotech.circular.point.")
                || a.starts_with("geotech.circular.interface.")
                || a.starts_with("geotech.circular.material.")
                || a.starts_with("geotech.circular.region.")
        })
}
fn require_runtime(b: &DocumentBinding) -> Result<(), String> {
    if b.document_uuid.trim().is_empty() || b.session_id.trim().is_empty() {
        Err("Нужен Document UUID / session / open-generation".into())
    } else {
        Ok(())
    }
}
fn validate_ground_record(g: &GroundSection) -> Result<(), String> {
    if g.ground_section_id.get() == 0
        || g.source_operation_id.get() < g.ground_section_id.get()
        || g.name.trim().is_empty()
    {
        return Err("Некорректный авторитетный GroundSection".into());
    }
    inspect_ground_readiness(&g.definition).map_err(|e| e.to_string())?;
    Ok(())
}
fn validate_case_record(c: &CircularCaseEditorProjection) -> Result<(), String> {
    let d = &c.definition;
    let s = &d.slip_surface;
    if c.analysis_id.get() == 0
        || c.source_operation_id.get() < c.analysis_id.get()
        || c.name.trim().is_empty()
        || c.name.len() > MAX_TEXT
        || d.ground_section_id.get() == 0
        || s.slip_id.0.trim().is_empty()
        || s.slip_id.0.len() > MAX_ID
        || !s.center.x.is_finite()
        || !s.center.z.is_finite()
        || !s.radius_m.is_finite()
        || s.radius_m <= 0.0
        || !d.extrusion_width_m.is_finite()
        || d.extrusion_width_m <= 0.0
        || d.units != InputUnits::default()
        || s.selected_endpoints.is_some_and(|p| {
            [p.entry_point, p.exit_point]
                .iter()
                .any(|v| !v.x.is_finite() || !v.z.is_finite())
        })
    {
        return Err("Некорректная проекция editor case".into());
    }
    validate_config(&d.config).map_err(|e| e.to_string())
}
fn section_snapshot(g: &GroundSection) -> GroundSection2D {
    GroundSection2D {
        section_id: GroundSectionId(g.ground_section_id.to_string()),
        model_revision: g.source_operation_id.get(),
        coordinate_system: g.definition.coordinate_system,
        terrain_polyline: g.definition.terrain_polyline.clone(),
        layer_interfaces: g.definition.layer_interfaces.clone(),
        soil_regions: g.definition.soil_regions.clone(),
        bottom_boundary: g.definition.bottom_boundary.clone(),
        materials: g.definition.materials.clone(),
    }
}
fn validate_resolved(
    g: &GroundSection,
    c: &CircularCaseEditorProjection,
    r: &ResolvedCircularAnalysis,
) -> Result<(), String> {
    validate_ground_record(g)?;
    validate_case_record(c)?;
    if !inspect_ground_readiness(&g.definition)
        .map_err(|e| e.to_string())?
        .is_ready()
    {
        return Err("GROUND_NOT_READY".into());
    }
    let d = &c.definition;
    if d.ground_section_id != g.ground_section_id
        || r.ground_source_operation_id != g.source_operation_id
        || r.case_source_operation_id != c.source_operation_id
        || r.config != d.config
        || r.input.analysis_id != c.analysis_id.to_string()
        || r.input.ground != section_snapshot(g)
        || r.input.source_revision
            != format!(
                "ground-operation:{}/case-operation:{}",
                g.source_operation_id, c.source_operation_id
            )
        || r.input.slip_surface != d.slip_surface
        || r.input.basis != d.basis
        || r.input.units != d.units
        || r.input.extrusion_width_m != d.extrusion_width_m
    {
        return Err("RESOLVED_GROUND_CASE_CONFIG_BINDING_MISMATCH".into());
    }
    Ok(())
}
fn upsert_ground(
    list: &mut Vec<Arc<GroundSection>>,
    value: Arc<GroundSection>,
) -> Result<(), String> {
    if let Some(old) = list
        .iter_mut()
        .find(|g| g.ground_section_id == value.ground_section_id)
    {
        *old = value;
    } else {
        if list.len() >= 256 {
            return Err("Лимит списка GroundSection".into());
        }
        list.push(value);
    }
    Ok(())
}
fn upsert_case(
    list: &mut Vec<Arc<CircularCaseEditorProjection>>,
    value: Arc<CircularCaseEditorProjection>,
) -> Result<(), String> {
    if let Some(old) = list.iter_mut().find(|c| c.analysis_id == value.analysis_id) {
        *old = value;
    } else {
        if list.len() >= 1024 {
            return Err("Лимит списка cases".into());
        }
        list.push(value);
    }
    Ok(())
}
fn check_receipt(
    expected: &DocumentBinding,
    live: &DocumentBinding,
    command_id: &CommandId,
    r: &CommitReceipt,
) -> Result<(), String> {
    require_runtime(live)?;
    if !expected.same_runtime(live)
        || r.replayed
        || &r.command_id != command_id
        || &r.transaction_id != command_id
        || r.operation_ids.len() != 1
        || r.revision_before != expected.revision
        || expected.revision.checked_add(1) != Some(r.revision_after)
        || r.revision != r.revision_after
        || live.revision != r.revision_after
    {
        return Err("RECEIPT_CONTEXT_MISMATCH / REPLAY_REQUIRES_CURRENT_RELOAD".into());
    }
    Ok(())
}
fn ground_command_matches(p: &SaveGroundProposal, g: &GroundSection, r: &CommitReceipt) -> bool {
    if p.expected_ground!=p.before.as_ref().map(|b|(b.ground_section_id,b.source_operation_id))
        || !r.affected_objects.iter().any(|v|matches!(v,ProjectObjectRef::GroundSection{ground_section_id} if *ground_section_id==g.ground_section_id)){return false;}
    let Some(op) = r.operation_ids.first() else {
        return false;
    };
    match (&p.command, p.before.as_deref()) {
        (EngineeringCommand::CreateGround { definition, name }, None) => {
            g.ground_section_id.get() == op.get()
                && g.source_operation_id == *op
                && &g.definition == definition
                && &g.name == name
                && r.delta
                    .engineering
                    .created_ground_sections
                    .contains(&g.ground_section_id)
        }
        (
            EngineeringCommand::UpdateGround {
                ground_section_id,
                definition,
                name,
            },
            Some(before),
        ) => {
            *ground_section_id == before.ground_section_id
                && g.ground_section_id == *ground_section_id
                && &g.definition == definition
                && g.name == name.as_ref().unwrap_or(&before.name).as_str()
                && g.source_operation_id
                    == if before.definition == *definition {
                        before.source_operation_id
                    } else {
                        *op
                    }
        }
        _ => false,
    }
}
fn case_command_matches(
    p: &SaveCaseProposal,
    c: &CircularCaseEditorProjection,
    r: &CommitReceipt,
) -> bool {
    if p.expected_case!=p.before.as_ref().map(|b|(b.analysis_id,b.source_operation_id))
        || !r.affected_objects.iter().any(|v|matches!(v,ProjectObjectRef::AnalysisCase{analysis_id} if *analysis_id==c.analysis_id)){return false;}
    let Some(op) = r.operation_ids.first() else {
        return false;
    };
    match (&p.command, p.before.as_deref()) {
        (EngineeringCommand::CreateCircular { definition, name }, None) => {
            c.analysis_id.get() == op.get()
                && c.source_operation_id == *op
                && &c.definition == definition
                && &c.name == name
                && r.delta.engineering.created_cases.contains(&c.analysis_id)
        }
        (
            EngineeringCommand::UpdateCircular {
                analysis_id,
                definition,
                name,
            },
            Some(before),
        ) => {
            *analysis_id == before.analysis_id
                && c.analysis_id == *analysis_id
                && &c.definition == definition
                && c.name == name.as_ref().unwrap_or(&before.name).as_str()
                && c.source_operation_id
                    == if before.definition == *definition {
                        before.source_operation_id
                    } else {
                        *op
                    }
        }
        _ => false,
    }
}
fn bounded(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.into();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[Показано не всё; полный набор доступен через dispatcher/report]",
        &s[..end]
    )
}
fn set_labels(ui: &WidgetRef, cx: &mut Cx, id: LiveId, labels: Vec<String>) {
    let dropdown = ui.drop_down(cx, &[id]);
    let selected = dropdown.selected_item().min(labels.len().saturating_sub(1));
    dropdown.set_labels(
        cx,
        if labels.is_empty() {
            vec!["Нет строк".into()]
        } else {
            labels
        },
    );
    // Pinned set_labels does not clamp; selected_label directly indexes.
    dropdown.set_selected_item(cx, selected);
}
fn text(ui: &WidgetRef, cx: &mut Cx, id: LiveId, max: usize) -> Result<String, String> {
    let value = ui.text_input(cx, &[id]).text();
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max {
        return Err(format!("Обязательное поле {id:?}: от 1 до {max} байт"));
    }
    Ok(trimmed.into())
}
fn parse_number(value: &str, name: &str) -> Result<f64, String> {
    let value = value.trim().replace(',', ".");
    let n = value
        .parse::<f64>()
        .map_err(|_| format!("{name}: введите число; текст сохранён"))?;
    if !n.is_finite() {
        return Err(format!("{name}: NaN/Infinity недопустимы; текст сохранён"));
    }
    Ok(n)
}
fn number(ui: &WidgetRef, cx: &mut Cx, id: LiveId, name: &str) -> Result<f64, String> {
    parse_number(&ui.text_input(cx, &[id]).text(), name)
}
fn positive(ui: &WidgetRef, cx: &mut Cx, id: LiveId, name: &str) -> Result<f64, String> {
    let n = number(ui, cx, id, name)?;
    if n <= 0.0 {
        Err(format!("{name}: требуется значение > 0"))
    } else {
        Ok(n)
    }
}
fn integer(ui: &WidgetRef, cx: &mut Cx, id: LiveId, name: &str) -> Result<usize, String> {
    ui.text_input(cx, &[id])
        .text()
        .trim()
        .parse()
        .map_err(|_| format!("{name}: требуется целое неотрицательное число"))
}
fn search_target_method(index: usize) -> Result<TargetMethod, String> {
    match index {
        1 => Ok(TargetMethod::Fellenius),
        2 => Ok(TargetMethod::Bishop),
        3 => Ok(TargetMethod::Both),
        _ => Err("Выберите целевой метод поиска".into()),
    }
}
fn search_analysis_config(config: &CircularAnalysisConfig) -> SearchAnalysisConfig {
    SearchAnalysisConfig {
        tolerances: config.tolerances,
        limits: config.limits,
        bishop: config.bishop,
        contact: config.contact,
    }
}
/// The durable policy intentionally omits shared case config and owner source
/// identity; the caller must compare those against the actual saved case/input.
fn circular_search_spec(request: &SearchPublicRequestV1) -> CircularSearchSpecV1 {
    CircularSearchSpecV1 {
        schema_version: request.schema_version,
        domain: request.domain.clone(),
        family_policy: request.family_policy,
        target_method: request.target_method,
        exploration_slices: request.exploration_slices,
        final_pair: request.final_pair,
        refinement: request.refinement,
        sensitivity: request.sensitivity.clone(),
        core_policy_profile: request.core_policy_profile,
        output_mode: request.output_mode,
        max_candidates: request.max_candidates,
        max_wall_time_ms: request.max_wall_time_ms,
        near_tie_abs: request.near_tie_abs,
        near_tie_rel: request.near_tie_rel,
    }
}
fn parse_u32(value: &str, name: &str) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("{name}: требуется целое число 0..{}", u32::MAX))
}
fn axis_points(axis: InclusiveAxis) -> Result<u64, String> {
    if !axis.min.is_finite() || !axis.max.is_finite() || axis.min > axis.max {
        return Err("Границы оси должны быть конечными; мин ≤ макс".into());
    }
    if axis.min == axis.max {
        return if axis.intervals == 0 {
            Ok(1)
        } else {
            Err("Фиксированная координата: мин=макс, интервалов=0".into())
        };
    }
    if axis.intervals == 0 {
        return Err("Между разными границами требуется положительное число интервалов".into());
    }
    let spacing = (axis.max - axis.min) / f64::from(axis.intervals);
    if !spacing.is_finite()
        || spacing <= 0.0
        || axis.min + spacing <= axis.min
        || axis.max - spacing >= axis.max
    {
        return Err("Разбиение неразличимо при данных координатах; core проверит все точки".into());
    }
    Ok(u64::from(axis.intervals) + 1)
}
fn search_matches_input(r: &SearchPublicRequestV1, i: &CircularAnalysisInput) -> bool {
    r.search_id == i.analysis_id
        && r.source_revision == i.source_revision
        && r.ground == i.ground
        && r.basis == i.basis
        && r.units == i.units
        && r.direction == i.slip_surface.slide_direction
        && r.extrusion_width_m == i.extrusion_width_m
}
fn search_box(
    ui: &WidgetRef,
    cx: &mut Cx,
    config: &CircularAnalysisConfig,
    input: &CircularAnalysisInput,
    template: Option<&SearchPublicRequestV1>,
) -> Result<SearchBox, String> {
    let mut r = template.cloned().ok_or("SEARCH_POLICY_NOT_LOADED · host должен передать полный типизированный запрос, включая общую пару и sensitivity")?;
    let mut axis = |a, b, c, name| -> Result<InclusiveAxis, String> {
        Ok(InclusiveAxis {
            min: number(ui, cx, a, name)?,
            max: number(ui, cx, b, name)?,
            intervals: parse_u32(&text(ui, cx, c, MAX_ID)?, name)?,
        })
    };
    r.domain.center_x = axis(
        id!(gcc_xmin),
        id!(gcc_xmax),
        id!(gcc_xintervals),
        "Интервалы X",
    )?;
    r.domain.center_z = axis(
        id!(gcc_zmin),
        id!(gcc_zmax),
        id!(gcc_zintervals),
        "Интервалы Z",
    )?;
    r.domain.radius = axis(
        id!(gcc_rmin),
        id!(gcc_rmax),
        id!(gcc_rintervals),
        "Интервалы R",
    )?;
    let mut range = |lo, hi, name| -> Result<Option<ClosedXRange>, String> {
        let lo = text(ui, cx, lo, MAX_ID)?;
        let hi = text(ui, cx, hi, MAX_ID)?;
        if lo.trim().is_empty() && hi.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(ClosedXRange {
            min_m: parse_number(&lo, name)?,
            max_m: parse_number(&hi, name)?,
        }))
    };
    r.domain.allowed_entry_x = range(id!(gcc_entry_xmin), id!(gcc_entry_xmax), "Вход X")?;
    r.domain.allowed_exit_x = range(id!(gcc_exit_xmin), id!(gcc_exit_xmax), "Выход X")?;
    r.max_candidates = integer(ui, cx, id!(gcc_candidates_limit), "Лимит кандидатов")? as u64;
    r.max_wall_time_ms = integer(ui, cx, id!(gcc_time_limit), "Лимит времени")? as u64;
    r.target_method =
        search_target_method(ui.drop_down(cx, ids!(gcc_search_target)).selected_item())?;
    r.refinement.levels = parse_u32(
        &text(ui, cx, id!(gcc_search_levels), MAX_ID)?,
        "Уровни уточнения",
    )?;
    r.refinement.promising_per_method = parse_u32(
        &text(ui, cx, id!(gcc_search_neighborhoods), MAX_ID)?,
        "Областей на метод",
    )?;
    let max_slices = integer(ui, cx, id!(gcc_search_max_slices), "Лимит отсеков поиска")?;
    r.shared_analysis = SearchAnalysisConfig {
        tolerances: config.tolerances,
        limits: config.limits,
        bishop: config.bishop,
        contact: config.contact,
    };
    r.shared_analysis.limits.max_slices = max_slices;
    r.search_id = input.analysis_id.clone();
    r.source_revision = input.source_revision.clone();
    r.ground = input.ground.clone();
    r.basis = input.basis;
    r.units = input.units;
    r.direction = input.slip_surface.slide_direction;
    r.extrusion_width_m = input.extrusion_width_m;
    let d = SearchBox {
        request: r,
        analysis_config: *config,
    };
    validate_search_box(&d)?;
    Ok(d)
}
/// Editor preflight only. Core owns complete streamed admission/resource policy.
fn validate_search_box(d: &SearchBox) -> Result<(), String> {
    let r = &d.request;
    let config = &d.analysis_config;
    validate_config(config).map_err(|e| e.to_string())?;
    if r.schema_version != 1
        || r.search_id.trim().is_empty()
        || r.source_revision.trim().is_empty()
        || r.search_id.len() > 256
        || r.source_revision.len() > 256
    {
        return Err("Неполный SearchPublicRequestV1".into());
    }
    let mut shared = r.shared_analysis;
    shared.limits.max_slices = config.limits.max_slices;
    if shared
        != (SearchAnalysisConfig {
            tolerances: config.tolerances,
            limits: config.limits,
            bishop: config.bishop,
            contact: config.contact,
        })
        || r.shared_analysis.limits.max_slices > config.limits.max_slices
    {
        return Err("SEARCH_CONFIG_MISMATCH · нет скрытого ослабления config".into());
    }
    validate_config(
        &r.shared_analysis
            .fixed(r.exploration_slices.initial_requested),
    )
    .map_err(|e| e.to_string())?;
    if r.max_candidates == 0
        || r.max_candidates > MAX_CANDIDATE_ROWS as u64
        || r.max_wall_time_ms == 0
        || r.max_wall_time_ms > 600_000
    {
        return Err("Лимиты этого редактора: 1..1000 кандидатов, 1..600000 мс; core допускает отдельный более широкий профиль".into());
    }
    let mut count = 1_u64;
    for axis in [r.domain.center_x, r.domain.center_z, r.domain.radius] {
        count = count
            .checked_mul(axis_points(axis)?)
            .filter(|n| *n <= r.max_candidates)
            .ok_or("Inclusive-сетка превышает лимит кандидатов")?;
    }
    if r.domain.radius.min <= 0.0 {
        return Err("Минимальный радиус должен быть положительным".into());
    }
    for range in [r.domain.allowed_entry_x, r.domain.allowed_exit_x]
        .into_iter()
        .flatten()
    {
        if !range.min_m.is_finite() || !range.max_m.is_finite() || range.min_m > range.max_m {
            return Err("Требуются конечные замкнутые X-диапазоны входа/выхода".into());
        }
    }
    if r.refinement.levels > 8
        || r.refinement.promising_per_method > 16
        || (r.refinement.levels > 0 && r.refinement.promising_per_method < 2)
    {
        return Err("Уточнение: уровней 0..8; при уточнении 2..16 областей на метод".into());
    }
    let e = r.exploration_slices;
    if e.initial_requested == 0
        || !(2..=16).contains(&e.max_passes)
        || !e.require_joint_consecutive_convergence
    {
        return Err(
            "Host exploration policy: 2..16 проходов и совместная последовательная сходимость"
                .into(),
        );
    }
    let lower = e
        .initial_requested
        .checked_mul(
            1_u32
                .checked_shl(e.max_passes - 1)
                .ok_or("Переполнение exploration")?,
        )
        .ok_or("Переполнение exploration")?;
    let pair_ok = |p: FinalComparisonPair, lower: u32| {
        p.coarse_requested > 0
            && p.coarse_requested >= lower
            && p.coarse_requested.checked_mul(2) == Some(p.fine_requested)
            && p.fine_requested as usize <= r.shared_analysis.limits.max_slices
    };
    if !pair_ok(r.final_pair, lower) {
        return Err(
            "Общая пара host: coarse ≥ exploration max, fine=2×coarse ≤ лимит отсеков".into(),
        );
    }
    let mut previous = r.final_pair.fine_requested;
    if r.sensitivity.additional_slice_pairs.len() > 8 {
        return Err("Слишком много дополнительных пар sensitivity".into());
    }
    for pair in &r.sensitivity.additional_slice_pairs {
        if !pair_ok(*pair, r.final_pair.fine_requested) || pair.fine_requested <= previous {
            return Err("Неупорядоченная host sensitivity slice policy".into());
        }
        previous = pair.fine_requested;
    }
    if [r.near_tie_abs, r.near_tie_rel]
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1e-2).contains(v))
    {
        return Err("Некорректная host near-tie policy".into());
    }
    Ok(())
}
fn persisted_search_ui_limit_error(request: &SearchPublicRequestV1) -> Option<String> {
    (request.max_candidates > MAX_CANDIDATE_ROWS as u64 || request.max_wall_time_ms > 600_000)
        .then(|| {
            "SEARCH_POLICY_CORE_ACCEPTED_UI_LIMIT · сохранённая политика не изменена; этот интерфейс поддерживает не более 1000 кандидатов и 600000 мс".into()
        })
}
fn accepted_search_state(
    status: &SearchRunStatus,
    conclusion: &SearchConclusion,
    accepted: bool,
    diagnostic: bool,
    mode: SearchOutputMode,
) -> bool {
    *status == SearchRunStatus::CompletedMinimum
        && *conclusion == SearchConclusion::MinimumWithinExaminedCircularFamily
        && accepted
        && !diagnostic
        && mode == SearchOutputMode::Strict
}
fn search_is_accepted(r: &SearchResultV1) -> bool {
    accepted_search_state(
        &r.status,
        &r.conclusion,
        r.accepted,
        r.diagnostic_only,
        r.request.public.output_mode,
    ) && !r.examined_family.eligible_candidate_ids.is_empty()
}
fn candidate_can_apply(r: &SearchResultV1, c: &CandidateSummary) -> bool {
    search_is_accepted(r)
        && c.main_family_member
        && c.final_status == CandidateStatus::JointlyEligible
        && c.bishop.whole_arc_status == Some(WholeArcContactStatus::PassedDeclaredModel)
        && r.examined_family.eligible_candidate_ids.contains(&c.id)
        && c.final_input_fingerprint
            .as_ref()
            .is_some_and(|v| !v.is_empty())
        && r.retained_tables.iter().any(|t| {
            t.candidate_id == c.id
                && t.source_fingerprint == r.source_fingerprint
                && t.config
                    == r.request
                        .public
                        .shared_analysis
                        .fixed(r.request.public.final_pair.fine_requested)
                && c.final_input_fingerprint.as_ref() == Some(&t.evaluation.input_fingerprint)
                && t.evaluation.bishop.as_ref().is_ok_and(|b| {
                    b.physical_assessment.status == BishopPhysicalStatus::Admissible
                        && b.endpoint_assessment.as_ref().is_some_and(|e| e.passed)
                        && b.whole_arc_contact.as_ref().is_some_and(|a| {
                            declared_contact_is_bound(a, &r.source_fingerprint, &t.config.contact)
                        })
                })
        })
        && [&c.fellenius, &c.bishop].iter().all(|m| {
            m.accepted_factor.is_some_and(|f| f.is_finite() && f > 0.0)
                && m.selected_family_eligibility.state == CheckState::Passed
        })
}
fn overlay_result_eligible(result: &CircularSlopeResult, review: &ResultReview, config: &CircularAnalysisConfig) -> bool {
            result.accepted
                && !result.diagnostic_only
                && result.bishop_result.physical_assessment.status
                    == BishopPhysicalStatus::Admissible
                && result.bishop_result.convergence_status == BishopConvergenceStatus::Converged
                && result.bishop_result.accepted_factor_of_safety.is_some()
                && result
                    .bishop_result
                    .endpoint_assessment
                    .as_ref()
                    .is_some_and(|a| a.passed)
                && review.finite_table.physical_status == PhysicalStatus::Admissible
                && review.whole_arc_admissible()
                && result
                    .bishop_result
                    .whole_arc_contact
                    .as_ref()
                    .is_some_and(|r| {
                        declared_contact_is_bound(
                            r,
                            &result.source_fingerprint,
                            &config.contact,
                        ) && review.whole_arc_applicability.as_deref()
                            == Some(r.scope.as_str())
                    })
                && review.geometry_endpoints.physical_status == PhysicalStatus::Admissible
                && review.fellenius.physical_status == PhysicalStatus::Admissible
                && review.bishop.physical_status == PhysicalStatus::Admissible
}
fn declared_contact_is_bound(
    r: &WholeArcContactReport,
    source: &str,
    config: &ContactScreenConfig,
) -> bool {
    r.status == WholeArcContactStatus::PassedDeclaredModel
        && r.coverage_complete
        && !r.scope.trim().is_empty()
        && !r.root_range_scope.trim().is_empty()
        && !r.contact_input_fingerprint.is_empty()
        && r.source_fingerprint == source
        && &r.config == config
    // Deliberately does not promote independent_whole_arc_acceptance or
    // eligible_for_critical_minimum: Phase B carries its separate owner policy.
}
fn validate_typed_search(
    r: &SearchResultV1,
    d: &SearchBox,
    i: &CalculationIdentity,
    config: &CircularAnalysisConfig,
) -> Result<(), String> {
    validate_search_box(d)?;
    if d.analysis_config != *config
        || r.schema_version != 1
        || r.request.public != d.request
        || r.source_fingerprint != i.source_fingerprint
        || r.solver_version != i.solver_version
        || r.request_fingerprint.is_empty()
        || r.candidates.len() as u64 > d.request.max_candidates
        || r.candidates.len() > MAX_CANDIDATE_ROWS
    {
        return Err("SEARCH_RESULT_BINDING_MISMATCH".into());
    }
    let ids: BTreeSet<_> = r.candidates.iter().map(|c| c.id).collect();
    if ids.len() != r.candidates.len()
        || r.candidates.iter().any(|c| {
            !c.center.x.is_finite()
                || !c.center.z.is_finite()
                || !c.radius_m.is_finite()
                || c.radius_m <= 0.0
        })
    {
        return Err("Некорректные кандидаты SearchResultV1".into());
    }
    if !search_is_accepted(r) {
        if r.minima.fellenius.is_some() || r.minima.bishop.is_some() {
            return Err(
                "Diagnostic/empty/incomplete результат не может публиковать accepted minima".into(),
            );
        }
        return Ok(());
    }
    for (method, minimum) in [
        (
            CircularMethod::FelleniusOrdinary,
            r.minima.fellenius.as_ref(),
        ),
        (CircularMethod::BishopSimplified, r.minima.bishop.as_ref()),
    ] {
        let m = minimum.ok_or("CompletedMinimum без отдельного минимума каждого метода")?;
        let row = r
            .candidates
            .iter()
            .find(|c| c.id == m.candidate_id)
            .ok_or("Минимум без исходного кандидата")?;
        let observation = if method == CircularMethod::FelleniusOrdinary {
            &row.fellenius
        } else {
            &row.bishop
        };
        if m.method != method
            || !candidate_can_apply(r, row)
            || !m.fine_factor.is_finite()
            || m.fine_factor <= 0.0
            || observation.accepted_factor != Some(m.fine_factor)
            || m.final_pair != r.request.public.final_pair
            || m.table_hash.is_empty()
            || m.evaluation_key.is_empty()
            || !r.retained_tables.iter().any(|t| {
                t.candidate_id == m.candidate_id
                    && t.table_hash == m.table_hash
                    && t.evaluation_key == m.evaluation_key
                    && t.source_fingerprint == r.source_fingerprint
                    && t.config
                        == r.request
                            .public
                            .shared_analysis
                            .fixed(m.final_pair.fine_requested)
                    && row.final_input_fingerprint.as_ref() == Some(&t.evaluation.input_fingerprint)
            })
        {
            return Err(
                "Минимум не связан с typed joint-eligible candidate / общей парой / retained table"
                    .into(),
            );
        }
    }
    Ok(())
}
fn search_policy_text(
    request: Option<&SearchPublicRequestV1>,
    config: &CircularAnalysisConfig,
    persisted: bool,
    owner_resolved: bool,
) -> String {
    let mut text = format!(
        "Политика в case: {}; запрос разрешён из Document: {}. Config: отсеков={}, максимум={}. Пределы этого UI: 1000 кандидатов / 600000 мс.",
        if persisted {
            "сохранена"
        } else {
            "не сохранена"
        },
        if owner_resolved { "да" } else { "нет" },
        config.slices.target_slices,
        config.limits.max_slices,
    );
    if let Some(r) = request {
        let _ = write!(
            text,
            "\nПолная typed policy: schema={} mode={:?} family={:?} target={:?}; exploration initial_requested={} max_passes={} require_joint_consecutive_convergence={}; final_pair coarse_requested={} fine_requested={}; refinement levels={} promising_per_method={}; sensitivity uniform_grid_bisections={} additional_slice_pairs={:?}; near_tie abs={} rel={}; caps max_candidates={} max_wall_time_ms={} ({}.{:03} s); profile={:?} output_mode={:?}. Domain: center_x=[{}, {}] intervals={}; center_z=[{}, {}] intervals={}; radius=[{}, {}] intervals={}; allowed_entry_x={:?}; allowed_exit_x={:?}. Заморозка — локальный предпросмотр; отдельная кнопка сохраняет политику в case.",
            r.schema_version,
            r.mode,
            r.family_policy,
            r.target_method,
            r.exploration_slices.initial_requested,
            r.exploration_slices.max_passes,
            r.exploration_slices.require_joint_consecutive_convergence,
            r.final_pair.coarse_requested,
            r.final_pair.fine_requested,
            r.refinement.levels,
            r.refinement.promising_per_method,
            r.sensitivity.uniform_grid_bisections,
            r.sensitivity.additional_slice_pairs,
            r.near_tie_abs,
            r.near_tie_rel,
            r.max_candidates,
            r.max_wall_time_ms,
            r.max_wall_time_ms / 1_000,
            r.max_wall_time_ms % 1_000,
            r.core_policy_profile,
            r.output_mode,
            r.domain.center_x.min,
            r.domain.center_x.max,
            r.domain.center_x.intervals,
            r.domain.center_z.min,
            r.domain.center_z.max,
            r.domain.center_z.intervals,
            r.domain.radius.min,
            r.domain.radius.max,
            r.domain.radius.intervals,
            r.domain.allowed_entry_x,
            r.domain.allowed_exit_x,
        );
    } else {
        text.push_str("\nПолный typed request не загружен; настройте его через host policy editor. Скрытые значения не подставляются.");
    }
    text
}
fn minima_text(r: &SearchResultV1) -> String {
    if !search_is_accepted(r) {
        return format!(
            "SEARCH_DOMAIN_LIMITED\n{:?} / {:?}\nНет accepted critical surface; диагностические best-so-far не являются минимумами",
            r.status, r.conclusion
        );
    }
    let mut text = DisplayText::new(32 * 1024);
    let _ = writeln!(
        text,
        "SEARCH_DOMAIN_LIMITED\nminimum_within_examined_circular_family · JointlyEligibleV1"
    );
    for (name, minimum) in [
        ("Феллениус", &r.minima.fellenius),
        ("Бишоп", &r.minima.bishop),
    ] {
        if text.truncated {
            break;
        }
        if let Some(m) = minimum {
            let _ = writeln!(
                text,
                "{}: кандидат {} · Fs={} · общая пара {:?} · {}",
                name, m.candidate_id, m.fine_factor, m.final_pair, m.label
            );
        } else {
            let _ = writeln!(text, "{name}: NOT_PROVIDED");
        }
    }
    let different = matches!((&r.minima.fellenius,&r.minima.bishop),(Some(f),Some(b)) if f.candidate_id!=b.candidate_id);
    let _ = writeln!(
        text,
        "{}\nОбласть host: {}\nНезависимая поисковая/инженерная приёмка этим выводом не предоставляется",
        if different {
            "Разные критические окружности по методам"
        } else {
            "Совпадение minima не является глобальной гарантией"
        },
        r.prerequisite_provenance.accepted_scope
    );
    text.finish()
}
/// Stop formatting at the display budget, including one oversized Debug field.
struct DisplayText {
    text: String,
    payload_limit: usize,
    notice: &'static str,
    truncated: bool,
}
impl DisplayText {
    fn new(limit: usize) -> Self {
        const NOTICE: &str = "\n[Показано не всё; полные типизированные данные сохранены host.]";
        let mut end = limit.min(NOTICE.len());
        while !NOTICE.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            text: String::new(),
            payload_limit: limit - end,
            notice: &NOTICE[..end],
            truncated: false,
        }
    }
    fn finish(mut self) -> String {
        if self.truncated {
            self.text.push_str(self.notice);
        }
        self.text
    }
}
impl std::fmt::Write for DisplayText {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        if self.truncated {
            return Err(std::fmt::Error);
        }
        let remaining = self.payload_limit.saturating_sub(self.text.len());
        if value.len() <= remaining {
            self.text.push_str(value);
            return Ok(());
        }
        let mut end = remaining;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        self.truncated = true;
        Err(std::fmt::Error)
    }
}
fn search_result_text(r: &SearchResultV1) -> String {
    let mut text = DisplayText::new(128 * 1024);
    if !search_is_accepted(r) {
        let _ = writeln!(
            text,
            "NO_ACCEPTED_CRITICAL_SURFACE; ниже только наблюдения методов"
        );
    }
    let _ = writeln!(
        text,
        "{:?} · {:?} · accepted={} diagnostic={}\nrequest={}\nsource={} solver={}\nFamily: {:?} · {}\nHost scope: {}\nReview: {} / {}\nProfile: {}\nCounters: {:?}\nBudgets: {:?}\nRefinement: {:?}\nWarnings: {:?}\nTerminal: {:?}\n",
        r.status,
        r.conclusion,
        r.accepted,
        r.diagnostic_only,
        r.request_fingerprint,
        r.source_fingerprint,
        r.solver_version,
        r.family_policy,
        r.examined_family.classification,
        r.prerequisite_provenance.accepted_scope,
        r.prerequisite_provenance.decision,
        r.prerequisite_provenance.review_status,
        r.request.profile_status,
        r.counters,
        r.request.budgets,
        r.refinement,
        r.warnings,
        r.terminal_reason
    );
    let p = &r.request.public;
    let _ = writeln!(
        text,
        "Численное окружение: {:?}\nExecution: {} · determinism={}\nPrerequisite receipt: {} · SHA256={}\nFamily binding={} · proposals={} · eligible-set={} · main count={} eligible count={}",
        r.arithmetic_environment,
        r.execution_policy,
        r.parallel_determinism,
        r.prerequisite_provenance.receipt_path,
        r.prerequisite_provenance.receipt_sha256,
        r.examined_family.binding_version,
        r.examined_family.proposal_manifest_hash,
        r.examined_family.eligible_set_hash,
        r.examined_family.main_candidate_ids.len(),
        r.examined_family.eligible_candidate_ids.len()
    );
    let _ = writeln!(
        text,
        "Сохранённая политика результата: target={:?}; exploration={:?}; общая пара={:?}; sensitivity plan={:?}; profile={:?}; output={:?}; near-tie abs={} rel={}",
        p.target_method,
        p.exploration_slices,
        p.final_pair,
        p.sensitivity,
        p.core_policy_profile,
        p.output_mode,
        p.near_tie_abs,
        p.near_tie_rel
    );
    for s in &r.sensitivity {
        if text.truncated {
            break;
        }
        let _ = writeln!(
            text,
            "Sensitivity {} / {}: {} · пара {:?} · eligible={} · winner_changed={:?} · ΔFs={:?} · eligibility_changed={} · warnings={:?}",
            s.branch,
            s.kind,
            s.status,
            s.final_pair,
            s.eligible_candidate_ids.len(),
            s.winner_changed,
            s.minimum_factor_delta,
            s.eligibility_changed,
            s.warnings
        );
    }
    for c in &r.candidates {
        if text.truncated {
            break;
        }
        let _ = writeln!(
            text,
            "{}: C=({},{}), R={} · {:?}\nНаблюдение Феллениуса raw={:?}, собственный Fs метода={:?}, family={:?}\nНаблюдение Бишопа raw={:?}, собственный Fs метода={:?}, convergence={:?}, finite={:?}, endpoints={:?}, whole_arc={:?} (только declared model, не unconditional PASS)\nФильтры {:?}/{:?}; bounds={:?}; причины={:?}",
            c.id,
            c.center.x,
            c.center.z,
            c.radius_m,
            c.final_status,
            c.fellenius.raw_factor,
            c.fellenius.accepted_factor,
            c.fellenius.selected_family_eligibility.state,
            c.bishop.raw_factor,
            c.bishop.accepted_factor,
            c.bishop.bishop_convergence_status,
            c.bishop.bishop_physical_status,
            c.bishop.endpoint_statuses,
            c.bishop.whole_arc_status,
            c.entry_filter.state,
            c.exit_filter.state,
            c.boundary,
            c.reasons
        );
    }
    text.finish()
}
fn endpoint_summary(b: &BishopResult) -> String {
    match &b.endpoint_assessment {
        None => "Core geometry endpoints: UNKNOWN / NOT_ASSESSED\n".into(),
        Some(assessment) => {
            let mut text = format!(
                "Core geometry endpoints: passed={} legacy_whole_arc_flag={} (не whole-arc assessment)\n",
                assessment.passed, assessment.whole_arc_contact_verified
            );
            for endpoint in &assessment.endpoints {
                let _ = writeln!(
                    text,
                    "  {:?}: {:?}, σN_limit={:?} кПа; {}",
                    endpoint.endpoint_type,
                    endpoint.status,
                    endpoint.normal_stress_limit_kpa,
                    endpoint.reason
                );
            }
            text
        }
    }
}
fn summary(result: &CircularSlopeResult, review: &ResultReview) -> String {
    let b = &result.bishop_result;
    let mut text = format!(
        "Core accepted={} · DIAGNOSTIC_ONLY={}\n",
        result.accepted, result.diagnostic_only
    );
    let _ = writeln!(
        text,
        "Fellenius equation F={:.8}\nФизический статус review: {:?}\n{}\nНезависимо проверен: {}",
        result.fellenius_result.factor_of_safety,
        review.fellenius.physical_status,
        review.fellenius.explanation,
        review.fellenius.independently_verified
    );
    let _ = writeln!(
        text,
        "\nBishop equation F={:.8} · {:?}\nЭто численный корень, не подтверждение контакта.\nДопустимый Fs ядра: {:?}\nКонтакт: {:?}; min N_B={} кН/м\nИтераций={} residual={} relative={} step={}",
        b.factor_of_safety,
        b.convergence_status,
        b.accepted_factor_of_safety,
        b.physical_assessment.status,
        b.physical_assessment.minimum_normal_kn_per_m,
        b.iterations.len(),
        b.equation_residual,
        b.relative_residual,
        b.final_step_delta
    );
    let _ = writeln!(
        text,
        "Физический статус review: {:?}\n{}\nНезависимо проверен: {}\nA={} м²; W={} кН/м; отсеков={}",
        review.bishop.physical_status,
        review.bishop.explanation,
        review.bishop.independently_verified,
        result.total_area_m2,
        result.total_weight_kn_per_m,
        result.slices.len()
    );
    let _ = writeln!(
        text,
        "Geometry endpoint guard: {:?}\n{}\nПоложительный N на конечных отсеках не подтверждает контакт у входа.",
        review.geometry_endpoints.physical_status, review.geometry_endpoints.explanation
    );
    let _ = writeln!(
        text,
        "Сгущение проверено: {}; ΔF={:?}; ΔB={:?}\nsource revision={}\ninput={}\nsource={}\nsolver={}\nCore numerically_verified={} · engineering_review_required={}\nПредупреждения: {}",
        result.discretization.refinement_checked,
        result.discretization.factor_delta,
        result.discretization.bishop_factor_delta,
        result.source_revision,
        result.input_fingerprint,
        result.source_fingerprint,
        result.solver_version,
        result.numerically_verified,
        result.engineering_review_required,
        result.warnings.join("; ")
    );
    text.push_str(&endpoint_summary(b));
    let _ = writeln!(text, "Whole-arc core (declared model only): {}", b.whole_arc_contact.as_ref().map_or_else(
        || "NOT_ASSESSED / HOST_REVIEW_REQUIRED".into(),
        |r| format!("{:?}; scope={}; root range={}; coverage_complete={}; independent={}; critical_eligible={}; не unconditional PASS",
            r.status,r.scope,r.root_range_scope,r.coverage_complete,r.independent_whole_arc_acceptance,r.eligible_for_critical_minimum)
    ));
    let _ = writeln!(
        text,
        "Finite-table review: {:?}\n{}\nWhole-arc review: {:?}\n{}\nОбласть применимости whole arc: {}\nWhole-arc независимая верификация: {}",
        review.finite_table.physical_status,
        review.finite_table.explanation,
        review.whole_arc_status(),
        review.whole_arc.explanation,
        review
            .whole_arc_applicability
            .as_deref()
            .unwrap_or("UNKNOWN / NOT_ASSESSED"),
        review.whole_arc.independently_verified
    );
    text
}
fn slice_page(result: &CircularSlopeResult, page: usize) -> String {
    let mut text = String::from(
        "X,Z,b,l — м; A — м²; W,N,D,R — кН/м; c′ — кПа; φ′/α — рад\nЗнаки сохраняются. N_B не вычисляется интерфейсом.\n",
    );
    for slice in result
        .slices
        .iter()
        .skip(page.saturating_mul(PAGE_SIZE))
        .take(PAGE_SIZE)
    {
        let _ = writeln!(
            text,
            "#{} X=[{},{}] b={} A={} W={} α={} chord={} arc={} base={}",
            slice.slice_id,
            slice.x_left,
            slice.x_right,
            slice.horizontal_width_m,
            slice.total_area_m2,
            slice.weight_kn_per_m,
            slice.base_inclination_rad,
            slice.base_length_m,
            slice.arc_length_m,
            slice.base_material_id.0
        );
        let _ = writeln!(
            text,
            "  A(material): {}",
            slice
                .area_by_material
                .iter()
                .map(|m| format!("{}={}", m.material_id.0, m.area_m2))
                .collect::<Vec<_>>()
                .join("; ")
        );
        if let Some(f) = result
            .fellenius_result
            .slice_contributions
            .iter()
            .find(|c| c.slice_id == slice.slice_id)
        {
            let _ = writeln!(
                text,
                "  Fellenius: c′={} φ′={} N={} D={} cohesion={} friction={} R={}",
                f.mechanics.effective_cohesion_kpa,
                f.mechanics.effective_friction_angle_rad,
                f.normal_kn_per_m,
                f.driving_kn_per_m,
                f.cohesion_kn_per_m,
                f.friction_kn_per_m,
                f.resistance_kn_per_m
            );
        } else {
            let _ = writeln!(text, "  Fellenius: MISSING_CONTRIBUTION");
        }
        if let Some(b) = result
            .bishop_result
            .slice_contributions
            .iter()
            .find(|c| c.slice_id == slice.slice_id)
        {
            let _ = writeln!(
                text,
                "  Bishop: mα={} numerator={} contribution={}",
                b.m_alpha, b.numerator_kn_per_m, b.contribution_kn_per_m
            );
            let f = &b.base_forces;
            let _ = writeln!(
                text,
                "  Bishop N_B={} T_mobilized={} S_available={} physical={:?}",
                f.normal_kn_per_m,
                f.mobilized_shear_kn_per_m,
                f.available_shear_strength_kn_per_m,
                f.physical_status
            );
            let _ = writeln!(
                text,
                "  N roundoff={} N bracket-min={} vertical residual={} bound={} shear-law residual={} bound={}",
                f.normal_roundoff_bound_kn_per_m,
                f.minimum_bracket_normal_kn_per_m,
                f.vertical_residual_kn_per_m,
                f.vertical_residual_bound_kn_per_m,
                f.shear_law_residual_kn_per_m,
                f.shear_law_residual_bound_kn_per_m
            );
        } else {
            let _ = writeln!(text, "  Bishop: MISSING_CONTRIBUTION");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation_id(value: u64) -> OperationId {
        serde_json::from_value(serde_json::json!(value))
            .expect("synthetic exact u64 operation identity")
    }

    fn synthetic_test_config() -> CircularAnalysisConfig {
        // The complete default config is only a deterministic synthetic fixture.
        CircularAnalysisConfig::default()
    }

    fn ready_ground_definition() -> GroundDefinition {
        let material_id = SoilMaterialId("synthetic-material".into());
        GroundDefinition {
            coordinate_system: SectionCoordinateSystem::CartesianXzMetres,
            terrain_polyline: vec![Point2::new(-10.0, 0.0), Point2::new(10.0, 0.0)],
            layer_interfaces: vec![],
            soil_regions: vec![SoilRegion {
                region_id: SoilRegionId("synthetic-region".into()),
                material_id: material_id.clone(),
            }],
            bottom_boundary: vec![Point2::new(-10.0, -10.0), Point2::new(10.0, -10.0)],
            materials: vec![GroundMaterial {
                material_id,
                properties: SoilMaterial {
                    name: "Synthetic test soil".into(),
                    unit_weight_kn_per_m3: 20.0,
                    effective_cohesion_kpa: 10.0,
                    effective_friction_angle_deg: 30.0,
                    provenance: "Synthetic unit-test fixture only".into(),
                },
            }],
        }
    }

    fn ground_record(id: u64, source: u64, name: &str) -> GroundSection {
        GroundSection {
            ground_section_id: DocumentGroundId::new(id),
            name: name.into(),
            definition: ready_ground_definition(),
            source_operation_id: operation_id(source),
        }
    }

    fn case_record(
        id: u64,
        source: u64,
        ground_id: DocumentGroundId,
        name: &str,
        config: CircularAnalysisConfig,
    ) -> CircularCaseEditorProjection {
        CircularCaseEditorProjection {
            analysis_id: AnalysisCaseId::new(id),
            name: name.into(),
            definition: CircularCaseDefinition {
                ground_section_id: ground_id,
                slip_surface: CircularSlipSurface::from_center_radius(
                    CircularSlipId(format!("synthetic-circle-{id}")),
                    Point2::new(0.0, -4.0),
                    6.0,
                    SlideDirection::PositiveX,
                )
                .expect("valid synthetic circle definition"),
                basis: AnalysisBasis::DryStatic,
                units: InputUnits::default(),
                extrusion_width_m: 1.0,
                config,
                search_spec: None,
            },
            source_operation_id: operation_id(source),
        }
    }

    fn resolved_for(
        ground: &GroundSection,
        case: &CircularCaseEditorProjection,
    ) -> ResolvedCircularAnalysis {
        let d = &case.definition;
        ResolvedCircularAnalysis {
            input: CircularAnalysisInput {
                analysis_id: case.analysis_id.to_string(),
                source_revision: format!(
                    "ground-operation:{}/case-operation:{}",
                    ground.source_operation_id, case.source_operation_id
                ),
                ground: section_snapshot(ground),
                slip_surface: d.slip_surface.clone(),
                basis: d.basis,
                units: d.units,
                extrusion_width_m: d.extrusion_width_m,
            },
            config: d.config,
            ground_source_operation_id: ground.source_operation_id,
            case_source_operation_id: case.source_operation_id,
        }
    }

    fn synthetic_binding() -> DocumentBinding {
        DocumentBinding {
            document_uuid: "synthetic-document".into(),
            session_id: "synthetic-session".into(),
            open_generation: 3,
            revision: 7,
        }
    }

    fn ready_panel() -> CircularPanel {
        let config = synthetic_test_config();
        let ground = Arc::new(ground_record(101, 1_001, "Synthetic ground"));
        let case = Arc::new(case_record(
            201,
            2_001,
            ground.ground_section_id,
            "Synthetic case",
            config,
        ));
        let resolved = resolved_for(&ground, &case);
        let binding = synthetic_binding();
        let mut panel = CircularPanel::default();
        panel.attach_document(binding.clone());
        panel.draft_ground = Some(ground.definition.clone());
        panel.saved_ground = Some(ground.clone());
        panel.saved_case = Some(case.clone());
        panel.ground_pick = Some(ground.ground_section_id);
        panel.case_pick = Some(case.analysis_id);
        panel.case_ground_ref = Some(ground.ground_section_id);
        panel.case_config_loaded = true;
        panel.config = resolved.config;
        panel
            .set_catalog(binding, vec![ground], vec![case])
            .expect("same-document synthetic catalog");
        let resolve_context = panel.editor_context().expect("synthetic editor context");
        panel
            .bind_resolved(
                &resolve_context,
                Arc::new(resolved),
                "synthetic-input-fingerprint".into(),
            )
            .expect("owner snapshot matches both synthetic source stamps");
        panel
    }

    fn test_search_domain(
        input: &CircularAnalysisInput,
        config: &CircularAnalysisConfig,
    ) -> SearchBox {
        let initial = u32::try_from(config.slices.target_slices)
            .expect("synthetic test slice count fits the public search type");
        let request = SearchPublicRequestV1 {
            schema_version: 1,
            mode: SearchMode::SearchCriticalCircle,
            search_id: input.analysis_id.clone(),
            source_revision: input.source_revision.clone(),
            ground: input.ground.clone(),
            basis: input.basis,
            units: input.units,
            direction: input.slip_surface.slide_direction,
            extrusion_width_m: input.extrusion_width_m,
            domain: SearchDomain {
                center_x: InclusiveAxis {
                    min: -1.0,
                    max: 1.0,
                    intervals: 2,
                },
                center_z: InclusiveAxis {
                    min: -5.0,
                    max: -3.0,
                    intervals: 2,
                },
                radius: InclusiveAxis {
                    min: 5.0,
                    max: 7.0,
                    intervals: 2,
                },
                allowed_entry_x: Some(ClosedXRange {
                    min_m: -10.0,
                    max_m: 0.0,
                }),
                allowed_exit_x: Some(ClosedXRange {
                    min_m: 0.0,
                    max_m: 10.0,
                }),
            },
            family_policy: FamilyPolicy::JointlyEligibleV1,
            target_method: TargetMethod::Both,
            shared_analysis: SearchAnalysisConfig {
                tolerances: config.tolerances,
                limits: config.limits,
                bishop: config.bishop,
                contact: config.contact,
            },
            exploration_slices: ExplorationSlices {
                initial_requested: initial,
                max_passes: 3,
                require_joint_consecutive_convergence: true,
            },
            final_pair: FinalComparisonPair {
                coarse_requested: initial * 4,
                fine_requested: initial * 8,
            },
            refinement: NeighborhoodRefinement {
                levels: 0,
                promising_per_method: 2,
            },
            sensitivity: SensitivityPlan {
                uniform_grid_bisections: 0,
                additional_slice_pairs: vec![],
            },
            core_policy_profile: CorePolicyProfile::ProvisionalSerialV1,
            output_mode: SearchOutputMode::Strict,
            max_candidates: 27,
            max_wall_time_ms: 1_000,
            near_tie_abs: 1e-10,
            near_tie_rel: 1e-10,
        };
        SearchBox {
            request,
            analysis_config: *config,
        }
    }

    fn bound_panel() -> CircularPanel {
        let mut panel = ready_panel();
        let input = panel
            .saved_input
            .as_deref()
            .expect("resolved fixture input");
        let search = test_search_domain(input, &panel.config);
        let mut case = panel.saved_case.as_deref().unwrap().clone();
        case.definition.search_spec = Some(circular_search_spec(&search.request));
        let ground = panel.saved_ground.as_deref().unwrap();
        if let Some(slot) = panel
            .cases
            .iter_mut()
            .find(|c| c.analysis_id == case.analysis_id)
        {
            *slot = Arc::new(case.clone());
        }
        panel.saved_case = Some(Arc::new(case.clone()));
        panel.search_request_template = Some(search.request.clone());
        panel.saved_search_domain = Some(search);
        panel.resolved_search = Some(ResolvedCircularSearch {
            request: panel.search_request_template.clone().unwrap(),
            ground_source_operation_id: ground.source_operation_id,
            case_source_operation_id: case.source_operation_id,
        });
        // The fixture represents explicit owner grants for both independent modes.
        grant_test_host_gates(&mut panel, true, true);
        panel
    }

    fn grant_test_host_gates(panel: &mut CircularPanel, analyze: bool, search: bool) {
        let context = panel.editor_context().expect("synthetic editor context");
        panel
            .set_host_gates(&context, analyze, search)
            .expect("gate decision is bound to the current synthetic context");
    }

    /// Explicitly generated synthetic archive data; authority state isolates the
    /// panel lifecycle from the separately tested Core inspector. These values
    /// are setup for lifecycle assertions, not independent numerical goldens.
    fn archived_fixture_panel(state: ArchiveState) -> (CircularPanel, Arc<ArchiveInspection>) {
        use crate::geotech_circular_archive_view::ArchivePresentation;
        let path = ai_revit_core::test_fixtures::ensure_circular_archive_fixtures()
            .join("specified.archive.json");
        let archive = geotech_report::decode_archive(
            &std::fs::read(path).expect("explicitly generated specified archive"),
            geotech_report::ReportLimits::default(),
        ).expect("persisted specified fixture");
        let context = &archive.bundle.context;
        let geotech_report::ReportEvidence::Specified { input, config, .. } = &archive.bundle.evidence else {
            panic!("specified fixture");
        };
        let ground = Arc::new(GroundSection {
            ground_section_id: DocumentGroundId::new(context.ground_id.0),
            name: "Saved fixture ground".into(),
            source_operation_id: operation_id(context.ground_source_operation.0),
            definition: GroundDefinition {
                coordinate_system: input.ground.coordinate_system,
                terrain_polyline: input.ground.terrain_polyline.clone(),
                layer_interfaces: input.ground.layer_interfaces.clone(),
                soil_regions: input.ground.soil_regions.clone(),
                bottom_boundary: input.ground.bottom_boundary.clone(),
                materials: input.ground.materials.clone(),
            },
        });
        let case = Arc::new(CircularCaseEditorProjection {
            analysis_id: AnalysisCaseId::new(context.case_id.0),
            name: "Saved fixture case".into(),
            source_operation_id: operation_id(context.case_source_operation.0),
            definition: CircularCaseDefinition {
                ground_section_id: ground.ground_section_id,
                slip_surface: input.slip_surface.clone(), basis: input.basis,
                units: input.units, extrusion_width_m: input.extrusion_width_m,
                config: *config, search_spec: None,
            },
        });
        let snapshot_wire = serde_json::json!({
            "schema_version":1,"archive":&archive,"source_case_definition":&case.definition
        }).to_string();
        let snapshot: ai_revit_core::CircularCalculationSnapshot = serde_json::from_str(
            &snapshot_wire,
        ).expect("bounded validated saved snapshot");
        let mut binding = synthetic_binding();
        binding.document_uuid = context.project_id.0.clone();
        binding.revision = context.captured_global_revision;
        let mut panel = CircularPanel::default();
        panel.attach_document(binding.clone());
        panel.draft_ground = Some(ground.definition.clone());
        panel.saved_ground = Some(ground.clone());
        panel.saved_case = Some(case.clone());
        panel.case_config_loaded = true;
        panel.case_ground_ref = Some(ground.ground_section_id);
        panel.config = *config;
        panel.set_catalog(binding, vec![ground.clone()], vec![case.clone()]).unwrap();
        let expected = panel.editor_context().unwrap();
        panel.bind_resolved(&expected, Arc::new(resolved_for(&ground, &case)), "saved-fixture-input".into()).unwrap();
        let inspection = Arc::new(ArchiveInspection {
            analysis_id: case.analysis_id,
            presentation: ArchivePresentation {
                state, origin: "document_archive", calculated_in_this_runtime: false,
                current_job_result: false, archive: None,
            },
            snapshot: Some(snapshot), report_preview: "Saved fixture report preview".into(),
        });
        let expected = panel.editor_context().unwrap();
        panel.accept_archive(&expected, inspection.clone(), ResultReview::default()).unwrap();
        (panel, inspection)
    }

    #[test]
    fn archived_values_are_visible_without_new_job_or_current_save_authority() {
        for state in [ArchiveState::ArchivedCurrent, ArchiveState::ArchivedStale] {
            let (panel, _) = archived_fixture_panel(state);
            let (_, _, saved) = panel.archived_specified().unwrap();
            let displayed = panel.display_result().unwrap();
            assert!(std::ptr::eq(displayed, saved));
            assert!(!displayed.slices.is_empty());
            assert_eq!(displayed.fellenius_result.factor_of_safety.to_bits(), saved.fellenius_result.factor_of_safety.to_bits());
            assert!(slice_page(displayed, 0).contains("N_B"));
            assert!(panel.archive_provenance_text().contains(&saved.solver_version));
            assert!(panel.current_result().is_none());
            assert!(panel.job.is_none());
            assert!(panel.result_source.is_none());
            assert!(panel.snapshot_save_ticket().is_none());
            assert!(panel.archived_for_overlays().is_none()); // saved worker arithmetic UNKNOWN
            assert!(panel.result_for_overlays().is_none());
            if state == ArchiveState::ArchivedStale {
                assert_eq!(panel.presentation_state(), "ARCHIVED_STALE");
                assert!(panel.archive_banner().contains("к текущим входам не применяются"));
            }
        }
    }

    #[test]
    fn archive_view_survives_unrelated_revision_but_draft_and_open_reject_late_reply() {
        let (mut panel, saved) = archived_fixture_panel(ArchiveState::ArchivedCurrent);
        let expected = panel.editor_context().unwrap();
        let ground = panel.saved_ground.clone().unwrap();
        let case = panel.saved_case.clone().unwrap();
        let mut live = expected.document.clone();
        live.revision += 1;
        panel.attach_document(live.clone());
        panel.observe_document(live.clone(), Some(&ground), Some(&case));
        assert!(panel.archived_view().is_some());
        panel.ground_changed();
        assert!(panel.archived_view().is_some());
        assert!(panel.display_result().is_some());
        assert_eq!(panel.presentation_state(), "ARCHIVED_STALE");
        assert!(panel.archive_banner().contains("локальный черновик изменён"));
        assert!(!panel.archive_matches_editor());
        assert!(panel.archived_for_overlays().is_none());
        assert!(panel.accept_archive(&expected, saved.clone(), ResultReview::default()).is_err());
        let draft = panel.draft_ground.clone();
        live.open_generation += 1;
        panel.attach_document(live);
        assert!(panel.archived_view().is_none());
        assert_eq!(panel.draft_ground, draft);
        assert!(panel.accept_archive(&expected, saved, ResultReview::default()).is_err());
    }

    #[test]
    fn archive_is_honest_history_during_calculation_and_failure() {
        let (mut panel, _) = archived_fixture_panel(ArchiveState::ArchivedCurrent);
        grant_test_host_gates(&mut panel, true, false);
        let ticket = panel.begin_job(JobKind::Analyze).unwrap();
        assert_eq!(panel.presentation_state(), "CALCULATING");
        assert!(panel.display_result().is_some());
        assert!(panel.archive_banner().starts_with("История"));
        panel.finish_error(&ticket, "synthetic failed completion");
        assert_eq!(panel.presentation_state(), "FAILED");
        assert!(panel.display_result().is_some());
        assert!(panel.current_result().is_none());
        assert!(panel.snapshot_save_ticket().is_none());
    }

    #[test]
    fn accepted_current_completion_takes_precedence_over_retained_archive() {
        let (mut panel, _) = archived_fixture_panel(ArchiveState::ArchivedCurrent);
        grant_test_host_gates(&mut panel, true, false);
        let ticket = panel.begin_job(JobKind::Analyze).unwrap();
        // Synthetic completion isolates UI precedence. Raw saved numeric values
        // are left intact; this does not claim a numerical or native host run.
        let mut completion = panel.archived_specified().unwrap().2.clone();
        completion.analysis_id = ticket.identity.analysis_id.to_string();
        completion.source_revision = ticket.identity.source_revision.clone();
        completion.input_fingerprint = ticket.identity.input_fingerprint.clone();
        completion.source_fingerprint = ticket.identity.source_fingerprint.clone();
        completion.solver_version = ticket.identity.solver_version.clone();
        let completion = Arc::new(completion);
        panel.accept_analysis(&ticket, completion.clone(), ResultReview::default()).unwrap();
        assert_eq!(panel.presentation_state(), "CURRENT_CALCULATED");
        assert!(std::ptr::eq(panel.display_result().unwrap(), completion.as_ref()));
        assert!(panel.archived_view().is_some());
        assert!(panel.archive_banner().starts_with("История"));
        assert!(panel.archived_for_overlays().is_none());
    }

    #[test]
    fn rejects_nonfinite_numeric_text_and_accepts_decimal_comma() {
        assert!(parse_number("NaN", "X").is_err());
        assert!(parse_number("inf", "X").is_err());
        assert_eq!(parse_number(" -1,25 ", "X").unwrap(), -1.25);
    }

    #[test]
    fn empty_ground_draft_validates_without_a_circle_or_case() {
        let draft = GroundDefinition::default();
        assert!(validate_ground_structure(&draft).is_ok());
        let readiness = inspect_ground_readiness(&draft).unwrap();
        assert_eq!(readiness.state, GroundReadinessState::Draft);
        assert!(!readiness.is_ready());
        assert!(
            readiness
                .missing
                .contains(&GroundMissingPart::TerrainPolyline)
        );
        assert!(
            !readiness
                .missing
                .contains(&GroundMissingPart::LayerInterfaces)
        ); // no undeclared interface is required in an empty draft
        assert!(readiness.missing.contains(&GroundMissingPart::SoilRegions));
        assert!(readiness.missing.contains(&GroundMissingPart::Materials));
        assert!(
            readiness
                .missing
                .contains(&GroundMissingPart::BottomBoundary)
        );
    }

    #[test]
    fn one_ground_resolves_two_cases_with_exact_source_references() {
        let config = synthetic_test_config();
        let ground = ground_record(101, 1_001, "Shared synthetic ground");
        let first = case_record(
            201,
            2_001,
            ground.ground_section_id,
            "First synthetic case",
            config,
        );
        let second = case_record(
            202,
            2_002,
            ground.ground_section_id,
            "Second synthetic case",
            config,
        );
        let first_resolved = resolved_for(&ground, &first);
        let second_resolved = resolved_for(&ground, &second);

        assert_eq!(first.definition.ground_section_id, ground.ground_section_id);
        assert_eq!(
            second.definition.ground_section_id,
            ground.ground_section_id
        );
        assert_eq!(
            first_resolved.input.source_revision,
            "ground-operation:1001/case-operation:2001"
        );
        assert_eq!(
            second_resolved.input.source_revision,
            "ground-operation:1001/case-operation:2002"
        );
        assert_eq!(
            first_resolved.ground_source_operation_id,
            ground.source_operation_id
        );
        assert_eq!(
            second_resolved.ground_source_operation_id,
            ground.source_operation_id
        );
        assert_eq!(
            first_resolved.case_source_operation_id,
            first.source_operation_id
        );
        assert_eq!(
            second_resolved.case_source_operation_id,
            second.source_operation_id
        );
        assert!(validate_resolved(&ground, &first, &first_resolved).is_ok());
        assert!(validate_resolved(&ground, &second, &second_resolved).is_ok());
    }

    #[test]
    fn operation_ids_above_json_f64_precision_are_preserved_exactly() {
        const BASE: u64 = 9_007_199_254_740_993;
        let ground = ground_record(BASE, BASE + 1, "Large-id synthetic ground");
        let case = case_record(
            BASE + 2,
            BASE + 3,
            ground.ground_section_id,
            "Large-id synthetic case",
            synthetic_test_config(),
        );
        let resolved = resolved_for(&ground, &case);

        assert_eq!(ground.ground_section_id.get(), BASE);
        assert_eq!(ground.source_operation_id.get(), BASE + 1);
        assert_eq!(case.analysis_id.get(), BASE + 2);
        assert_eq!(case.source_operation_id.get(), BASE + 3);
        assert_eq!(
            resolved.input.source_revision,
            format!("ground-operation:{}/case-operation:{}", BASE + 1, BASE + 3)
        );
        assert_eq!(resolved.ground_source_operation_id.get(), BASE + 1);
        assert_eq!(resolved.case_source_operation_id.get(), BASE + 3);
        assert!(validate_resolved(&ground, &case, &resolved).is_ok());
    }

    #[test]
    fn document_session_or_reopen_stales_and_preserves_the_local_draft() {
        for reopen in [false, true] {
            let mut panel = ready_panel();
            grant_test_host_gates(&mut panel, true, false);
            let ticket = panel.begin_job(JobKind::Analyze).unwrap();
            let mut local_draft = panel.draft_ground.clone().unwrap();
            local_draft.terrain_polyline[0].z += 0.25;
            panel.draft_ground = Some(local_draft.clone());
            panel.ground_changed();
            panel.pending_rows.insert("point");
            let draft_generation = panel.draft_generation;
            let mut live = panel.live_binding.clone().unwrap();
            if reopen {
                live.open_generation += 1;
            } else {
                live.session_id = "replacement-session".into();
            }
            let ground = panel.saved_ground.clone().unwrap();
            let case = panel.saved_case.clone().unwrap();

            panel.attach_document(live.clone());
            panel.observe_document(live, Some(&ground), Some(&case));

            assert!(panel.stale);
            assert!(panel.dirty);
            assert_eq!(panel.draft_generation, draft_generation);
            assert_eq!(panel.draft_ground.as_ref(), Some(&local_draft));
            assert!(panel.pending_rows.contains("point"));
            assert!(!panel.job_current(&ticket));
            assert!(panel.current_result().is_none());
        }
    }

    #[test]
    fn numeric_ground_source_change_stales_without_discarding_draft_or_pending_rows() {
        let mut panel = ready_panel();
        let mut local_draft = panel.draft_ground.clone().unwrap();
        local_draft.terrain_polyline[0].z += 0.25;
        panel.draft_ground = Some(local_draft.clone());
        panel.ground_changed();
        panel.pending_rows.insert("point");
        let draft_generation = panel.draft_generation;

        let mut remote_ground = (*panel.saved_ground.clone().unwrap()).clone();
        remote_ground.definition.terrain_polyline[1].z += 0.5;
        remote_ground.source_operation_id = operation_id(1_002);
        let case = panel.saved_case.clone().unwrap();
        let mut live = panel.live_binding.clone().unwrap();
        live.revision += 1;

        panel.observe_document(live, Some(&remote_ground), Some(&case));

        assert!(panel.stale);
        assert!(panel.ground_dirty);
        assert_eq!(panel.draft_generation, draft_generation);
        assert_eq!(panel.draft_ground.as_ref(), Some(&local_draft));
        assert!(panel.pending_rows.contains("point"));
    }

    #[test]
    fn either_owner_source_stamp_change_stales_the_bound_snapshot() {
        for change_ground in [false, true] {
            let mut panel = ready_panel();
            let mut ground = (*panel.saved_ground.clone().unwrap()).clone();
            let mut case = (*panel.saved_case.clone().unwrap()).clone();
            if change_ground {
                ground.source_operation_id = operation_id(1_002);
            } else {
                case.source_operation_id = operation_id(2_002);
            }
            let mut live = panel.live_binding.clone().unwrap();
            live.revision += 1;

            panel.observe_document(live, Some(&ground), Some(&case));

            assert!(panel.stale);
            assert!(!panel.analysis_gate_accepted);
            assert!(!panel.search_gate_accepted);
            assert!(panel.saved_input.is_some());
        }
    }

    #[test]
    fn rename_noop_and_unrelated_global_revision_keep_the_job_current() {
        let mut panel = ready_panel();
        grant_test_host_gates(&mut panel, true, false);
        let ticket = panel.begin_job(JobKind::Analyze).unwrap();
        let mut ground = (*panel.saved_ground.clone().unwrap()).clone();
        let mut case = (*panel.saved_case.clone().unwrap()).clone();
        let mut live = panel.live_binding.clone().unwrap();
        live.revision += 1;

        panel.observe_document(live.clone(), Some(&ground), Some(&case));
        assert!(panel.job_current(&ticket));

        ground.name = "Renamed ground".into();
        case.name = "Renamed case".into();
        live.revision += 1;
        panel.observe_document(live.clone(), Some(&ground), Some(&case));
        assert!(panel.job_current(&ticket));
        assert_eq!(panel.saved_ground.as_ref().unwrap().name, "Renamed ground");
        assert_eq!(panel.saved_case.as_ref().unwrap().name, "Renamed case");

        live.revision += 1;
        panel.observe_document(live, Some(&ground), Some(&case));
        assert!(panel.job_current(&ticket));
        assert!(!panel.stale);
    }

    #[test]
    fn local_config_edit_invalidates_an_existing_analysis_ticket() {
        let mut panel = ready_panel();
        grant_test_host_gates(&mut panel, true, false);
        let ticket = panel.begin_job(JobKind::Analyze).unwrap();
        panel.config.slices.target_slices += 1;
        panel.changed();

        assert!(!panel.job_current(&ticket));
        assert_eq!(panel.job.as_ref(), Some(&ticket));
        assert!(panel.case_dirty);
        assert!(!panel.ground_dirty);
    }

    #[test]
    fn owner_gates_are_required_independently_for_analyze_and_search() {
        let mut ungranted = ready_panel();
        assert!(
            ungranted
                .begin_job(JobKind::Analyze)
                .unwrap_err()
                .contains("HOST_GATE_NOT_ACCEPTED")
        );
        assert!(
            ungranted
                .begin_job(JobKind::Search)
                .unwrap_err()
                .contains("HOST_GATE_NOT_ACCEPTED")
        );
        assert!(ungranted.job.is_none());

        let mut analyze = ready_panel();
        grant_test_host_gates(&mut analyze, true, false);
        assert!(analyze.begin_job(JobKind::Analyze).is_ok());

        let mut search = bound_panel();
        search.analysis_gate_accepted = false;
        grant_test_host_gates(&mut search, false, true);
        assert!(
            search
                .begin_job(JobKind::Analyze)
                .unwrap_err()
                .contains("HOST_GATE_NOT_ACCEPTED")
        );
        assert_eq!(
            search.begin_job(JobKind::Search).unwrap().kind,
            JobKind::Search
        );
    }

    #[test]
    fn mode_tickets_pin_both_source_stamps_and_the_complete_search_spec() {
        let mut analyze = ready_panel();
        grant_test_host_gates(&mut analyze, true, false);
        let analyze_ticket = analyze.begin_job(JobKind::Analyze).unwrap();
        assert_eq!(analyze_ticket.kind, JobKind::Analyze);
        assert!(analyze_ticket.search_spec.is_none());
        assert_eq!(
            analyze_ticket.identity.ground_source_operation_id,
            analyze.saved_ground.as_ref().unwrap().source_operation_id
        );
        assert_eq!(
            analyze_ticket.identity.case_source_operation_id,
            analyze.saved_case.as_ref().unwrap().source_operation_id
        );

        let mut search = bound_panel();
        search.analysis_gate_accepted = false;
        let saved_spec = search.saved_search_domain.clone().unwrap();
        let search_ticket = search.begin_job(JobKind::Search).unwrap();
        assert_eq!(search_ticket.kind, JobKind::Search);
        assert_eq!(search_ticket.search_spec, Some(saved_spec.clone()));
        search
            .saved_search_domain
            .as_mut()
            .unwrap()
            .request
            .max_wall_time_ms += 1;
        assert!(!search.job_current(&search_ticket));
    }

    #[test]
    fn load_tickets_are_latest_wins_and_catalog_is_bound_to_one_document_session() {
        let mut panel = ready_panel();
        let ground = panel.saved_ground.clone().unwrap();
        let case = panel.saved_case.clone().unwrap();
        let live = panel.live_binding.clone().unwrap();
        let old = panel
            .record_load_ticket(
                RecordLoadMode::Case,
                ground.ground_section_id,
                Some(case.analysis_id),
            )
            .unwrap();
        let latest = panel
            .record_load_ticket(
                RecordLoadMode::Case,
                ground.ground_section_id,
                Some(case.analysis_id),
            )
            .unwrap();

        assert!(latest.context.load_generation > old.context.load_generation);
        assert!(
            panel
                .check_record_load(&old, &live, RecordLoadMode::Case, &ground, Some(&case))
                .is_err()
        );
        assert!(
            panel
                .check_record_load(&latest, &live, RecordLoadMode::Case, &ground, Some(&case))
                .is_ok()
        );

        let mut foreign_session = live.clone();
        foreign_session.session_id = "different-document-session".into();
        panel.attach_document(foreign_session.clone());
        let error = panel
            .record_load_ticket(
                RecordLoadMode::Case,
                ground.ground_section_id,
                Some(case.analysis_id),
            )
            .unwrap_err();
        assert!(error.contains("STALE_CATALOG"));
        assert!(
            panel
                .check_record_load(
                    &latest,
                    &foreign_session,
                    RecordLoadMode::Case,
                    &ground,
                    Some(&case)
                )
                .is_err()
        );
    }

    #[test]
    fn dirty_input_and_cancel_reject_old_job_publication() {
        let mut panel = ready_panel();
        grant_test_host_gates(&mut panel, true, false);
        let job = panel.begin_job(JobKind::Analyze).unwrap();
        assert!(panel.job_current(&job));
        panel.changed();
        assert!(!panel.job_current(&job));
        assert_eq!(panel.job.as_ref(), Some(&job));
        panel.case_dirty = false;
        panel.update_dirty();
        panel.cancelling = true;
        assert!(!panel.job_current(&job));
    }

    #[test]
    fn no_physical_acceptance_is_inferred_from_default_review() {
        let review = ResultReview::default();
        assert_eq!(
            review.fellenius.physical_status,
            PhysicalStatus::NotAssessed
        );
        assert_eq!(review.bishop.physical_status, PhysicalStatus::NotAssessed);
        assert!(!review.bishop.independently_verified);
    }

    #[test]
    fn overlays_cannot_publish_without_a_current_physically_reviewed_result() {
        let panel = CircularPanel::default();
        let flags = panel.effective_flags();
        assert!(flags.ground && flags.soil_layers);
        assert!(!flags.failure_mass && !flags.slices && !flags.forces && !flags.critical_circle);
    }

    #[test]
    fn byte_bound_never_splits_a_russian_character() {
        let text = bounded("Грунт", 3);
        assert!(text.starts_with("Г"));
        assert!(text.contains("Показано не всё"));
        let mut display = DisplayText::new(1024);
        let oversized = "Грунт".repeat(2048);
        assert!(write!(display, "{oversized}").is_err());
        let output = display.finish();
        assert!(output.len() <= 1024);
        assert!(output.contains("Показано не всё"));
        assert!(std::str::from_utf8(output.as_bytes()).is_ok());
    }

    #[test]
    fn search_grid_refuses_stalled_steps_excess_product_and_unbounded_time() {
        let panel = ready_panel();
        let input = panel.saved_input.as_deref().unwrap();
        let mut spec = test_search_domain(input, &panel.config);
        assert!(validate_search_box(&spec).is_ok());
        spec.request.domain.center_x = InclusiveAxis {
            min: 1e9,
            max: 1e9 + 1.0,
            intervals: u32::MAX,
        };
        assert!(validate_search_box(&spec).is_err());
        spec.request.domain.center_x = InclusiveAxis {
            min: 0.0,
            max: 3.0,
            intervals: 3,
        };
        assert!(validate_search_box(&spec).is_err());
        spec.request.domain.center_x = InclusiveAxis {
            min: -1.0,
            max: 1.0,
            intervals: 2,
        };
        spec.request.max_wall_time_ms = u64::MAX;
        assert!(validate_search_box(&spec).is_err());
    }

    #[test]
    fn cancel_racing_success_releases_only_the_finished_job() {
        let mut panel = bound_panel();
        let old = panel.begin_job(JobKind::Search).unwrap();
        panel.cancelling = true;
        assert!(panel.accept_candidates(&old, vec![]).is_err());
        assert!(panel.job.is_none());
        assert!(!panel.cancelling);
        let newer = panel.begin_job(JobKind::Search).unwrap();
        assert!(panel.accept_candidates(&old, vec![]).is_err());
        assert_eq!(panel.job.as_ref(), Some(&newer));
    }

    #[test]
    fn edited_search_bounds_reject_old_completion() {
        let mut panel = bound_panel();
        let job = panel.begin_job(JobKind::Search).unwrap();
        panel.search_generation += 1;
        assert!(!panel.job_current(&job));
        assert!(panel.accept_candidates(&job, vec![]).is_err());
        assert!(panel.job.is_none());
        assert!(panel.candidate_source.is_none());
    }

    #[test]
    fn stale_point_handle_cannot_modify_a_new_document_draft() {
        let mut panel = ready_panel();
        let mut old = panel.binding.clone().unwrap();
        old.open_generation -= 1;
        assert!(
            panel
                .drag_point(
                    &old,
                    panel.draft_generation,
                    BoundaryTarget::Terrain,
                    0,
                    Point2::new(3.0, 4.0)
                )
                .is_err()
        );
        assert!(!panel.dirty);
        assert!(panel.draft_ground.is_some());
    }

    #[test]
    fn semantic_aliases_and_native_ids_are_unique() {
        let mut aliases = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for (alias, id) in CIRCULAR_INPUTS
            .iter()
            .chain(CIRCULAR_BUTTONS)
            .chain(CIRCULAR_SELECTORS)
            .chain(CIRCULAR_TOGGLES)
            .chain(CIRCULAR_OUTPUTS)
        {
            assert!(aliases.insert(*alias), "duplicate alias {alias}");
            assert!(ids.insert(*id), "duplicate native ID {id:?}");
        }
    }

    #[test]
    fn finite_and_endpoint_pass_do_not_promote_whole_arc() {
        let mut review = ResultReview::default();
        review.finite_table.physical_status = PhysicalStatus::Admissible;
        review.geometry_endpoints.physical_status = PhysicalStatus::Admissible;
        assert_eq!(review.whole_arc_status(), PhysicalStatus::NotAssessed);
        assert!(!review.whole_arc_admissible());
        review.whole_arc.physical_status = PhysicalStatus::Admissible;
        assert_eq!(review.whole_arc_status(), PhysicalStatus::NotAssessed);
        review.whole_arc_applicability = Some("synthetic declared test model".into());
        assert!(review.whole_arc_admissible());
        assert!(!review.whole_arc.independently_verified);
    }

    #[test]
    fn search_domain_checks_entry_exit_ranges_and_method_choice() {
        let panel = ready_panel();
        let mut spec = test_search_domain(panel.saved_input.as_deref().unwrap(), &panel.config);
        assert!(validate_search_box(&spec).is_ok());
        spec.request.domain.allowed_entry_x = Some(ClosedXRange {
            min_m: 2.0,
            max_m: 1.0,
        });
        assert!(validate_search_box(&spec).is_err());
        spec.request.domain.allowed_entry_x = Some(ClosedXRange {
            min_m: 0.0,
            max_m: 1.0,
        });
        spec.request.domain.allowed_exit_x = Some(ClosedXRange {
            min_m: f64::NAN,
            max_m: 2.0,
        });
        assert!(validate_search_box(&spec).is_err());
        assert!(search_target_method(0).is_err());
        assert!(search_target_method(4).is_err());
        assert_eq!(search_target_method(1).unwrap(), TargetMethod::Fellenius);
        assert_eq!(search_target_method(2).unwrap(), TargetMethod::Bishop);
        assert_eq!(search_target_method(3).unwrap(), TargetMethod::Both);
    }

    #[test]
    fn search_refinement_and_slice_caps_do_not_weaken_config() {
        let panel = ready_panel();
        let mut spec = test_search_domain(panel.saved_input.as_deref().unwrap(), &panel.config);
        let original = spec.analysis_config;
        spec.request.refinement.levels = 9;
        assert!(validate_search_box(&spec).is_err());
        spec.request.refinement.levels = 8;
        assert!(validate_search_box(&spec).is_ok());
        spec.request.refinement.promising_per_method = 17;
        assert!(validate_search_box(&spec).is_err());
        spec.request.refinement.promising_per_method = 16;
        assert!(validate_search_box(&spec).is_ok());
        spec.request.shared_analysis.limits.max_slices = original.limits.max_slices + 1;
        assert!(validate_search_box(&spec).is_err());
        spec.request.shared_analysis.limits.max_slices = original.slices.target_slices - 1;
        assert!(validate_search_box(&spec).is_err());
        assert_eq!(spec.analysis_config, original);
        assert!(spec.analysis_config.slices.verify_refinement);
    }

    #[test]
    fn missing_or_changed_search_spec_blocks_search_with_explicit_state() {
        let mut panel = bound_panel();
        panel.saved_search_domain = None;
        let error = panel.begin_job(JobKind::Search).unwrap_err();
        assert!(error.contains("SEARCH_SPEC_NOT_FROZEN"));
        assert!(panel.job.is_none());

        let input = panel.saved_input.as_deref().unwrap();
        panel.saved_search_domain = Some(test_search_domain(input, &panel.config));
        panel.config.slices.target_slices += 1;
        let error = panel.begin_job(JobKind::Search).unwrap_err();
        assert!(error.contains("SEARCH_SPEC_NOT_FROZEN"));
        assert!(panel.job.is_none());
    }

    #[test]
    fn search_knobs_invalidate_search_ticket_without_dirtying_ground_or_case() {
        let mut panel = bound_panel();
        let job = panel.begin_job(JobKind::Search).unwrap();
        let draft_generation = panel.draft_generation;
        panel.candidate_source = Some(job.clone());
        panel.search_settings_changed();
        assert!(panel.dirty);
        assert!(panel.search_dirty);
        assert!(!panel.ground_dirty && !panel.case_dirty);
        assert_eq!(panel.draft_generation, draft_generation);
        assert!(panel.search_generation > job.search_generation);
        assert!(panel.search_result.is_none());
        assert!(panel.candidate_source.is_none());
        assert!(!panel.job_current(&job));
        assert!(panel.accept_candidates(&job, vec![]).is_err());
        assert!(panel.job.is_none());
        let error = panel.begin_job(JobKind::Search).unwrap_err();
        assert!(error.contains("HOST_GATE_NOT_ACCEPTED"));
    }

    #[test]
    fn incomplete_and_diagnostic_search_states_never_accept_minima() {
        for status in [
            SearchRunStatus::CompletedNoEligibleCandidates,
            SearchRunStatus::InvalidRequest,
            SearchRunStatus::PrerequisiteNotAccepted,
            SearchRunStatus::BudgetExhausted,
            SearchRunStatus::DeadlineExhausted,
            SearchRunStatus::Cancelled,
            SearchRunStatus::Stale,
            SearchRunStatus::InternalFailure,
        ] {
            assert!(!accepted_search_state(
                &status,
                &SearchConclusion::MinimumWithinExaminedCircularFamily,
                true,
                false,
                SearchOutputMode::Strict
            ));
        }
        assert!(accepted_search_state(
            &SearchRunStatus::CompletedMinimum,
            &SearchConclusion::MinimumWithinExaminedCircularFamily,
            true,
            false,
            SearchOutputMode::Strict
        ));
        assert!(!accepted_search_state(
            &SearchRunStatus::CompletedMinimum,
            &SearchConclusion::MinimumWithinExaminedCircularFamily,
            true,
            true,
            SearchOutputMode::Strict
        ));
        assert!(!accepted_search_state(
            &SearchRunStatus::CompletedMinimum,
            &SearchConclusion::MinimumWithinExaminedCircularFamily,
            true,
            false,
            SearchOutputMode::Diagnostic
        ));
        assert!(!accepted_search_state(
            &SearchRunStatus::CompletedMinimum,
            &SearchConclusion::NoStabilityEvidence,
            true,
            false,
            SearchOutputMode::Strict
        ));
    }

    #[test]
    fn inclusive_axes_keep_fixed_coordinates_and_both_declared_endpoints() {
        assert_eq!(
            axis_points(InclusiveAxis {
                min: 2.0,
                max: 2.0,
                intervals: 0
            })
            .unwrap(),
            1
        );
        assert!(
            axis_points(InclusiveAxis {
                min: 2.0,
                max: 2.0,
                intervals: 1
            })
            .is_err()
        );
        assert!(
            axis_points(InclusiveAxis {
                min: 0.0,
                max: 2.0,
                intervals: 0
            })
            .is_err()
        );
        assert_eq!(
            axis_points(InclusiveAxis {
                min: 0.0,
                max: 2.0,
                intervals: 2
            })
            .unwrap(),
            3
        );
        assert_eq!(
            axis_points(InclusiveAxis {
                min: 0.0,
                max: 2.0,
                intervals: 3
            })
            .unwrap(),
            4
        );
    }

    #[test]
    fn interval_input_rejects_fractional_step_text_and_u32_overflow() {
        assert!(parse_u32("0.25", "Интервалы").is_err());
        assert!(parse_u32("1.0", "Интервалы").is_err());
        assert!(parse_u32("-1", "Интервалы").is_err());
        assert!(parse_u32("4294967296", "Интервалы").is_err());
        assert_eq!(parse_u32("0", "Интервалы").unwrap(), 0);
        assert_eq!(parse_u32("4", "Интервалы").unwrap(), 4);
    }
    fn projection_test_view(case: &CircularCaseEditorProjection) -> CircularCaseEditorView<'_> {
        CircularCaseEditorView {
            analysis_id: case.analysis_id,
            name: &case.name,
            definition: &case.definition,
            source_operation_id: case.source_operation_id,
        }
    }
    #[test]
    fn case_editor_projection_wire_schema_preserves_owner_fields_and_escaped_bytes() {
        // This is the named editor schema, not a reconstructed Core case or an
        // assertion that its authoritative Document has no saved archive.
        let case = case_record(
            9_007_199_254_740_993,
            9_007_199_254_740_995,
            DocumentGroundId::new(7),
            "Разрез \"А\"\n",
            CircularAnalysisConfig::default(),
        );
        let view = projection_test_view(&case);
        let encoded = serde_json::to_vec(&view).unwrap();
        assert_eq!(view.serialized_bytes().unwrap(), encoded.len());
        let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            ["analysis_id", "name", "definition", "source_operation_id"]
                .into_iter()
                .collect()
        );
        assert_eq!(
            value["analysis_id"],
            serde_json::to_value(case.analysis_id).unwrap()
        );
        assert_eq!(
            value["source_operation_id"],
            serde_json::to_value(case.source_operation_id).unwrap()
        );
        assert_eq!(value["name"].as_str(), Some(case.name.as_str()));
        assert_eq!(serde_json::to_value(&case).unwrap(), value);
    }
    #[test]
    fn case_editor_projection_rejects_field_and_definition_overflow() {
        let mut case = case_record(
            20,
            22,
            DocumentGroundId::new(7),
            "case",
            CircularAnalysisConfig::default(),
        );
        case.name = "Ж".repeat(MAX_TEXT / 2 + 1);
        assert!(
            projection_test_view(&case)
                .serialized_bytes()
                .unwrap_err()
                .contains("CASE_EDITOR_NAME_BYTES")
        );
        case.name = "case".into();
        case.definition.slip_surface.slip_id.0 = "x".repeat(MAX_ID + 1);
        assert!(
            projection_test_view(&case)
                .serialized_bytes()
                .unwrap_err()
                .contains("CASE_EDITOR_SLIP_ID_BYTES")
        );
        // Deliberately malformed untrusted policy input must hit the streaming
        // byte gate before it could become an owned editor payload.
        let panel = bound_panel();
        let mut case = panel.saved_case.as_deref().unwrap().clone();
        case.definition
            .search_spec
            .as_mut()
            .unwrap()
            .sensitivity
            .additional_slice_pairs = vec![
            FinalComparisonPair {
                coarse_requested: 8,
                fine_requested: 16
            };
            10_000
        ];
        assert!(
            projection_test_view(&case)
                .serialized_bytes()
                .unwrap_err()
                .contains("CASE_EDITOR_DEFINITION_BYTES_EXCEEDED")
        );
    }
}
