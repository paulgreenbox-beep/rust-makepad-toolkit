use super::*;
use geotech_core::Point2;

fn line(w: &mut dyn Write, args: std::fmt::Arguments<'_>) -> Result<(), ReportError> {
    w.write_fmt(args).map_err(|_| ReportError::OutputBudget)?;
    w.write_all(b"\n").map_err(|_| ReportError::OutputBudget)
}
macro_rules! out { ($w:expr, $($arg:tt)*) => { line($w, format_args!($($arg)*))? }; }
fn text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        .replace('|', "&#124;").replace('`', "&#96;").replace('\r', " ").replace('\n', " ")
}
struct Indented<'a> { inner: &'a mut dyn Write, start: bool }
impl Write for Indented<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            if self.start { self.inner.write_all(b"    ")?; }
            self.inner.write_all(part)?;
            self.start = part.last() == Some(&b'\n');
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { self.inner.flush() }
}
fn json(w: &mut dyn Write, label: &str, value: &impl Serialize) -> Result<(), ReportError> {
    out!(w, "\n{} (полная сохранённая типизированная запись):\n", text(label));
    serde_json::to_writer_pretty(Indented { inner: w, start: true }, value)
        .map_err(|_| ReportError::OutputBudget)?;
    out!(w, "\n"); Ok(())
}
struct Table<'a> {
    label: String,
    geometry: &'a FailureMassGeometry,
    slices: &'a [Slice],
    area: f64,
    weight: f64,
    fellenius: Result<&'a FelleniusResult, &'a CircularError>,
    bishop: Result<&'a BishopResult, &'a CircularError>,
}
fn tables(bundle: &ReportBundle, mut f: impl FnMut(Table<'_>) -> Result<(), ReportError>)
    -> Result<(), ReportError> {
    match &bundle.evidence {
        ReportEvidence::Specified { result: r, .. } => f(Table {
            label: "Заданная окружность".into(), geometry: &r.geometry,
            slices: &r.slices, area: r.total_area_m2, weight: r.total_weight_kn_per_m,
            fellenius: Ok(&r.fellenius_result), bishop: Ok(&r.bishop_result) }),
        ReportEvidence::Search { result, .. } => {
            for t in &result.retained_tables {
                // Human report includes every row for each distinct main winner.
                // Other retained evidence remains lossless in the typed archive.
                if ![&result.minima.fellenius, &result.minima.bishop].into_iter()
                    .flatten().any(|m| m.candidate_id == t.candidate_id
                        && m.table_hash == t.table_hash && m.evaluation_key == t.evaluation_key) {
                    continue;
                }
                f(Table { label: format!("Кандидат {}; ветвь {}; {} slices requested; {}",
                    t.candidate_id, t.branch, t.config.slices.target_slices, t.evaluation_key),
                    geometry: &t.evaluation.geometry, slices: &t.evaluation.slices,
                    area: t.evaluation.total_area_m2, weight: t.evaluation.total_weight_kn_per_m,
                    fellenius: t.evaluation.fellenius.as_ref(), bishop: t.evaluation.bishop.as_ref() })?;
            }
            Ok(())
        }
    }
}
fn ground(bundle: &ReportBundle) -> &GroundSection2D {
    match &bundle.evidence {
        ReportEvidence::Specified { input, .. } => &input.ground,
        ReportEvidence::Search { request, .. } => &request.ground,
    }
}
fn points(w: &mut dyn Write, label: &str, points: &[Point2]) -> Result<(), ReportError> {
    out!(w, "\n{} — все точки X/Z, m:\n\n| № | X | Z |\n|---:|---:|---:|", text(label));
    for (i, p) in points.iter().enumerate() { out!(w, "| {} | {} | {} |", i, p.x, p.z); }
    Ok(())
}
pub(super) fn render(bundle: &ReportBundle, w: &mut dyn Write) -> Result<(), ReportError> {
    let context = &bundle.context;
    let ground = ground(bundle);
    out!(w, "# Инженерный отчёт по сохранённым данным круглоцилиндрического расчёта\n");
    out!(w, "Формат: {}. Содержимое проверяется отдельно от актуальности, независимой математической проверки и инженерной приёмки. Пересчёт при форматировании/открытии не выполняется.\n", FORMATTER_VERSION);
    out!(w, "## 1. Идентификаторы проекта и расчёта\n\nProject: {}; Case: {}; Ground: {}.\n", text(&context.project_id.0), context.case_id.0, context.ground_id.0);
    out!(w, "## 2. Ревизии и fingerprints\n\nGlobal revision (provenance): {}. Ground source operation: {}; Case source operation: {}.\n\nSource revision: {}.\n\nCalculation fingerprint: {}.\n\nSolver source fingerprint: {}.\n", context.captured_global_revision, context.ground_source_operation.0, context.case_source_operation.0, context.source_revision(), text(bundle.calculation_digest()), text(&context.archived_solver_source_fingerprint));
    out!(w, "## 3. Версия solver и сохранённая FP-среда\n\nSolver: {}. Digest algorithm: {}.\n", text(&context.archived_solver_version), INPUT_DIGEST_ALGORITHM);
    match &context.saved_calculation_worker_arithmetic {
        Some(report) => json(w, "Сохранённая среда фактического calculation worker (host)", report)?,
        None => out!(w, "Сохранённая FP-среда calculation worker (host): UNKNOWN / отсутствует. Formatter не создаёт заменяющего подтверждения.\n"),
    }
    if let ReportEvidence::Search { result, .. } = &bundle.evidence {
        json(w, "FP-среда фактического caller search core", &result.arithmetic_environment)?;
        json(w, "Условное owner prerequisite provenance, без повышения статуса", &result.prerequisite_provenance)?;
    }
    out!(w, "## 4. Геологический разрез\n\nCoordinate system: {:?}; model_revision: {}.\n", ground.coordinate_system, ground.model_revision);
    points(w, "Terrain", &ground.terrain_polyline)?;
    for interface in &ground.layer_interfaces {
        points(w, &format!("Layer interface {}", interface.interface_id.0), &interface.polyline)?;
    }
    points(w, "Bottom boundary", &ground.bottom_boundary)?;
    json(w, "Soil region/material assignment", &ground.soil_regions)?;
    out!(w, "## 5. Материалы\n\n| ID | Наименование | gamma kN/m³ | c′ kPa | phi′ deg | Provenance |\n|---|---|---:|---:|---:|---|");
    for material in &ground.materials {
        let p = &material.properties;
        out!(w, "| {} | {} | {} | {} | {} | {} |", text(&material.material_id.0), text(&p.name), p.unit_weight_kn_per_m3, p.effective_cohesion_kpa, p.effective_friction_angle_deg, text(&p.provenance));
    }
    out!(w, "\nОтсутствующие source references остаются отсутствующими; provenance материалов не считается независимой проверкой.\n");
    out!(w, "## 6. Окружность и направление сдвига\n");
    match &bundle.evidence {
        ReportEvidence::Specified { input, .. } => json(w, "Запрошенная круговая поверхность", &input.slip_surface)?,
        ReportEvidence::Search { result, .. } => {
            json(w, "Раздельные FF/Bishop winners (могут быть разными)", &result.minima)?;
            out!(w, "Не существует подставной originalcircle: ниже только реально сохранённые candidate tables. Отсутствующая winner table — ошибка формирования.\n");
        }
    }
    tables(bundle, |t| json(w, &format!("{}: разрешённая окружность и endpoints", t.label), &t.geometry.circle))?;
    out!(w, "## 7. Призма обрушения / failure mass\n");
    tables(bundle, |t| json(w, &t.label, t.geometry))?;
    out!(w, "## 8. Все slices\n\nSigned inclination и driving force сохраняют знак. Base length — chord, отдельно указана arc length. Единицы: m, m², kN/m; inclination rad.\n");
    tables(bundle, |t| {
        out!(w, "### {}\n\n| slice | Xleft | Xright | width | area | weight | alpha signed | base chord | arc | base material |\n|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|", text(&t.label));
        for s in t.slices {
            out!(w, "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |", s.slice_id, s.x_left, s.x_right, s.horizontal_width_m, s.total_area_m2, s.weight_kn_per_m, s.base_inclination_rad, s.base_length_m, s.arc_length_m, text(&s.base_material_id.0));
        }
        // Includes ALL per-material areas, base endpoint points and numerical quality.
        json(w, "Полные строки slices", &t.slices)
    })?;
    out!(w, "## 9. Площадь массы\n");
    tables(bundle, |t| { out!(w, "{}: total area = {} m²; geometry area = {} m².\n", text(&t.label), t.area, t.geometry.area_m2); Ok(()) })?;
    out!(w, "## 10. Вес и вклады материалов\n\nВклад веса ниже — арифметическое представление сохранённой material area × gamma; не дополнительный расчёт устойчивости.\n");
    tables(bundle, |t| {
        out!(w, "{}: saved total weight = {} kN/m.\n\n| Material | Area m² | gamma kN/m³ | Derived weight kN/m |\n|---|---:|---:|---:|", text(&t.label), t.weight);
        for a in &t.geometry.material_areas {
            let m = ground.materials.iter().find(|m| m.material_id == a.material_id)
                .ok_or(ReportError::Binding("material weight provenance"))?;
            let weight = a.area_m2 * m.properties.unit_weight_kn_per_m3;
            if !weight.is_finite() { return Err(ReportError::NonFinite); }
            out!(w, "| {} | {} | {} | {} |", text(&a.material_id.0), a.area_m2, m.properties.unit_weight_kn_per_m3, weight);
        }
        Ok(())
    })?;
    out!(w, "## 11. Fellenius / Ordinary\n");
    tables(bundle, |t| {
        match t.fellenius {
            Ok(r) => {
                out!(w, "{}: FF = {}; driving sum = {} kN/m; resisting sum = {} kN/m.\n\n| slice | normal | driving signed | cohesion | friction | resistance |\n|---:|---:|---:|---:|---:|---:|", text(&t.label), r.factor_of_safety, r.driving_sum_kn_per_m, r.resisting_sum_kn_per_m);
                for s in &r.slice_contributions {
                    out!(w, "| {} | {} | {} | {} | {} | {} |", s.slice_id, s.normal_kn_per_m, s.driving_kn_per_m, s.cohesion_kn_per_m, s.friction_kn_per_m, s.resistance_kn_per_m);
                }
                json(w, "Fellenius: все mechanics, силы и assumptions", r)
            }
            Err(error) => json(w, "Fellenius: сохранённая ошибка, accepted factor отсутствует", error),
        }
    })?;
    out!(w, "## 12. Bishop Simplified и все итерации\n");
    tables(bundle, |t| {
        match t.bishop {
            Ok(r) => {
                out!(w, "{}: raw factor = {}; accepted factor = {:?}; convergence = {:?}.\n\nInitial requested {}; initial used {}; admissible lower {}; driving sum {} kN/m; Bishop sum {} kN/m.\n\n| iteration | step | factor before | factor after | delta | residual | relative residual | min m_alpha | bracket lower | bracket upper |\n|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|", text(&t.label), r.factor_of_safety, r.accepted_factor_of_safety, r.convergence_status, r.initial_factor_requested, r.initial_factor_used, r.admissible_lower_factor, r.driving_sum_kn_per_m, r.bishop_sum_kn_per_m);
                for i in &r.iterations {
                    out!(w, "| {} | {:?} | {} | {} | {} | {} | {} | {} | {} | {} |", i.iteration, i.step, i.factor_before, i.factor_after, i.step_delta, i.equation_residual, i.relative_residual, i.minimum_m_alpha, i.bracket_lower, i.bracket_upper);
                }
                json(w, "Bishop: все slice contributions / mechanics / base forces", &r.slice_contributions)?;
                json(w, "Bishop assumptions", &r.assumptions)?;
                if r.whole_arc_contact.as_ref().is_some_and(|r| r.details_truncated) {
                    out!(w, "Сохранённый core whole-arc report имеет details_truncated=true: исходные подробности уже отсутствуют. Formatter ничего не восстанавливает и не повышает conditional status.\n");
                }
                Ok(())
            }
            Err(error) => json(w, "Bishop: ошибка/diagnostic, raw root не является accepted factor", error),
        }
    })?;
    out!(w, "## 13. Невязки, thresholds и условные статусы\n\nFinite-table, endpoint и whole-arc evidence остаются раздельными. Whole-arc условен round-to-nearest + gradual underflow на фактическом calculation thread. Независимый статус не повышается formatter.\n");
    tables(bundle, |t| {
        if let Ok(r) = t.bishop {
            out!(w, "{}: final step delta {}; step tolerance {}; equation residual {}; relative residual {}; residual tolerance {}.\n", text(&t.label), r.final_step_delta, r.step_tolerance, r.equation_residual, r.relative_residual, r.residual_tolerance);
            json(w, "Physical finite-table assessment", &r.physical_assessment)?;
            json(w, "Endpoint assessment (Option = missing means absent)", &r.endpoint_assessment)?;
            json(w, "Whole-arc conditional assessment (Option = missing means absent)", &r.whole_arc_contact)?;
        }
        Ok(())
    })?;
    out!(w, "## 14. Результат поиска критической окружности\n\nReport conclusion: {:?}. Это не глобальный или continuum minimum.\n", bundle.conclusion());
    if let ReportEvidence::Search { result, .. } = &bundle.evidence {
        out!(w, "Status {:?}; conclusion {:?}; saved accepted {}; diagnostic_only {}.\n", result.status, result.conclusion, result.accepted, result.diagnostic_only);
        json(w, "Minima отдельно по методам", &result.minima)?;
        json(w, "Diagnostic best-so-far — не accepted minimum", &result.diagnostic_best_so_far)?;
        json(w, "Joint family / manifest / domain-limited classification", &result.examined_family)?;
        out!(w, "Все candidate outcomes (полные proposal origins/key/pass records остаются в typed archive):\n\n| Candidate | Geometry | Final | FF raw | FF accepted | Bishop raw | Bishop accepted |\n|---:|---|---|---:|---:|---:|---:|");
        for c in &result.candidates {
            out!(w, "| {} | {:?} | {:?} | {:?} | {:?} | {:?} | {:?} |", c.id, c.geometry.state, c.final_status, c.fellenius.raw_factor, c.fellenius.accepted_factor, c.bishop.raw_factor, c.bishop.accepted_factor);
        }
        for c in &result.candidates {
            json(w, &format!("Candidate {}: все rejected/unresolved reasons", c.id),
                &(&c.reasons, &c.geometry.reason, &c.entry_filter.reason, &c.exit_filter.reason,
                    &c.fellenius.error, &c.bishop.error))?;
        }
        json(w, "Refinement и seeds", &result.refinement)?;
        json(w, "Counters / work", &result.counters)?;
        json(w, "Budgets / retention / effective policy", &(&result.request.budgets, &result.request.retention, &result.request.profile_status))?;
        json(w, "Terminal reason", &result.terminal_reason)?;
    } else { out!(w, "Поиск не выполнялся: предоставлен только результат заданной окружности.\n"); }
    out!(w, "## 15. Границы поиска, filters и точная спецификация\n");
    match &bundle.evidence {
        ReportEvidence::Search { request, .. } => {
            json(w, "Bounds и entry/exit filters", &request.domain)?;
            json(w, "Search config / slice exploration / final pair / refinement / sensitivity / policy / target / ties / limits", &(&request.shared_analysis, &request.exploration_slices, &request.final_pair, &request.refinement, &request.sensitivity, &request.family_policy, &request.target_method, &request.core_policy_profile, &request.output_mode, request.max_candidates, request.max_wall_time_ms, request.near_tie_abs, request.near_tie_rel))?;
        }
        ReportEvidence::Specified { input, config, .. } => {
            out!(w, "Search bounds отсутствуют (specified mode).\n");
            json(w, "Полный specified input", input)?;
            json(w, "Полный specified config, tolerances/limits/contact/Bishop", config)?;
        }
    }
    out!(w, "## 16. Сходимость slices и чувствительность\n");
    match &bundle.evidence {
        ReportEvidence::Specified { result, .. } => json(w, "Discretization / refinement evidence", &result.discretization)?,
        ReportEvidence::Search { result, .. } => {
            json(w, "Все sensitivity branches и раздельные method minima", &result.sensitivity)?;
            if result.sensitivity.is_empty() { out!(w, "Дополнительная sensitivity не сохранена / не запрошена. Конечная пара slices указана в request/minima; это не continuum proof.\n"); }
        }
    }
    out!(w, "## 17. Численные предупреждения\n");
    match &bundle.evidence {
        ReportEvidence::Specified { result, .. } => {
            json(w, "Warnings (не удалены)", &result.warnings)?;
            out!(w, "Saved flags: numerically_verified={}; accepted={}; diagnostic_only={}; engineering_review_required={}. Статусы сохранены, не подтверждены formatter.\n", result.numerically_verified, result.accepted, result.diagnostic_only, result.engineering_review_required);
        }
        ReportEvidence::Search { result, .. } => json(w, "Warnings (включая SEARCH_DOMAIN_LIMITED)", &result.warnings)?,
    }
    out!(w, "## 18. Применимость\n\nСухая статическая 2D-модель предельного равновесия без порового давления и внешних нагрузок.\n\nТолько self-weight, effective c′/phi′ и заданная круговая семья. 3D — только extrusion-визуализация; пространственная устойчивость и боковые силы не рассчитываются. Нет нормативного approval, инженерной приёмки или доказательства global/continuum minimum. Условные FP/контактные предпосылки solver сохраняются.\n");
    out!(w, "## 19. Источники\n");
    if bundle.source_references.is_empty() {
        out!(w, "Source references: MISSING. GEO5, лицензированный handbook и USACE не помечены VERIFIED; доступ/дата/страницы отсутствуют.\n");
    } else { json(w, "Все source references с реальным availability; проверенный статус не изобретается", &bundle.source_references)?; }
    out!(w, "## 20. Инженерная проверка\n\nENGINEERING_REVIEW_REQUIRED. SHA256 подтверждает согласованность сохранённых байтов/типизированных bindings, а не подлинность происхождения, актуальность текущего Document, корректность математики или инженерную пригодность. Отчёт не является native Document evidence или полной Gate8 acceptance.\n");
    out!(w, "Полный typed host context/input/config/result/search tuple сохранён один раз в companion archive; Markdown не дублирует JSON всего payload. Несохранённые строки не восстанавливаются пересчётом.\n");
    Ok(())
}
