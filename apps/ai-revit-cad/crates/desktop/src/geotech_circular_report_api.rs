//! Bounded report control. Formatting is asynchronous; export writes saved Markdown.
use super::*;
use geotech_circular_reportworker::ReportPhase;

fn rejected(code: &'static str) -> agent_handler::RequestFailure {
    agent_handler::RequestFailure::geometry_error(
        format!("Circular report rejected: {code}"),
        json!({"module":"geotech_circular_report","code":code}),
    )
}
fn number(params: &Value, name: &str) -> Result<u64, agent_handler::RequestFailure> {
    let text = params
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| rejected("InvalidDecimalId"))?;
    if text.len() > 20 {
        return Err(rejected("InvalidDecimalId"));
    }
    let value = text
        .parse::<u64>()
        .map_err(|_| rejected("InvalidDecimalId"))?;
    if value == 0 || value.to_string() != text {
        return Err(rejected("InvalidDecimalId"));
    }
    Ok(value)
}

impl App {
    /// Shared circular dispatcher has already checked context/token/wrapper caps.
    pub(super) fn circular_report_dispatch(
        &mut self,
        request: &agent_api::ApiCommand,
    ) -> Result<Value, agent_handler::RequestFailure> {
        let specific: &[&str] = match request.method.as_str() {
            "geotech.circular.report.create" => &["sequence"],
            "geotech.circular.report.status" => &["report_sequence"],
            "geotech.circular.report.export" => &["report_sequence", "path"],
            _ => return Err(rejected("UnknownMethod")),
        };
        let object = request
            .params
            .as_object()
            .ok_or_else(|| rejected("InvalidParams"))?;
        for key in object.keys() {
            if !specific.contains(&key.as_str())
                && ![
                    "expected_session_id",
                    "expected_document_id",
                    "expected_runtime_id",
                    "expected_open_generation",
                ]
                .contains(&key.as_str())
            {
                return Err(rejected("UnknownParameter"));
            }
        }
        self.circular_validate_report_compute();
        if request.method == "geotech.circular.report.create" {
            let sequence = circular_host::DecimalU64(number(&request.params, "sequence")?);
            let ctx = self.circular_context();
            let doc = self
                .document
                .as_ref()
                .ok_or_else(|| rejected("NoDocument"))?;
            let controller = self
                .circular_host
                .as_mut()
                .ok_or_else(|| rejected("Gone"))?;
            let status = controller
                .status(doc, &ctx, sequence)
                .map_err(|_| rejected("NotCurrentCompleted"))?;
            let saved = controller
                .current_result(doc, &ctx, sequence)
                .map_err(|_| rejected("NotCurrentCompleted"))?;
            let report_sequence = self
                .circular_report
                .start_from_saved(sequence.0, saved, status.captured_project_revision.0)
                .map_err(|error| {
                    rejected(match error {
                        geotech_circular_reportworker::ReportWorkError::Busy => "Busy",
                        geotech_circular_reportworker::ReportWorkError::ShuttingDown => {
                            "ShuttingDown"
                        }
                        geotech_circular_reportworker::ReportWorkError::InputBudget => {
                            "InputBudget"
                        }
                        geotech_circular_reportworker::ReportWorkError::ModeMismatch => {
                            "ModeMismatch"
                        }
                        geotech_circular_reportworker::ReportWorkError::SpawnFailed => {
                            "SpawnFailed"
                        }
                        geotech_circular_reportworker::ReportWorkError::SequenceExhausted => {
                            "SequenceExhausted"
                        }
                    })
                })?;
            self.circular_report_ui = geotech_circular_report::CircularReportUiState::default();
            return Ok(
                json!({"report_sequence":report_sequence.to_string(),"compute_sequence":sequence,
                "phase":"Running","engineering_review_required":true}),
            );
        }
        let sequence = number(&request.params, "report_sequence")?;
        if sequence != self.circular_report.sequence() {
            return Err(rejected("Gone"));
        }
        let phase = self.circular_report.phase();
        let ctx = self.circular_context();
        let doc = self
            .document
            .as_ref()
            .ok_or_else(|| rejected("NoDocument"))?;
        if request.method == "geotech.circular.report.status" {
            let details = self.circular_report.current(doc, &ctx).map(|(key, archive)| json!({
                "compute_sequence":key.compute_sequence.to_string(),"request_digest":key.request_digest,
                "content_digest":key.content_digest,"archive_digest":archive.archive_digest,
                "report_digest":archive.report_digest,"markdown_bytes":archive.markdown.len().to_string(),
                "archived_solver_version":archive.bundle.context.archived_solver_version,
                "archived_solver_source_fingerprint":archive.bundle.context.archived_solver_source_fingerprint,
            }));
            return Ok(
                json!({"report_sequence":sequence.to_string(),"phase":format!("{phase:?}"),
                "worker_owned":!self.circular_report.is_quiescent(),"current":phase==ReportPhase::Completed && details.is_some(),
                "details":details,"engineering_review_required":true}),
            );
        }
        let path = request
            .params
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| rejected("InvalidPath"))?;
        if path.is_empty()
            || path.len() > 4096
            || path.contains('\0')
            || !Path::new(path)
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            return Err(rejected("MarkdownPathRequired"));
        }
        let (key, archive) = self
            .circular_report
            .current(doc, &ctx)
            .ok_or_else(|| rejected("NotCurrentCompleted"))?;
        // The complete text was formatted/validated in the owned worker. No
        // report rendering/JSON archive encoding or solver runs on the UI owner.
        file_guard::FilePolicy::from_environment()
            .map_err(|_| rejected("FilePolicy"))?
            .write_atomic(Path::new(path), archive.markdown.as_bytes())
            .map_err(|_| rejected("ExportFailed"))?;
        Ok(
            json!({"exported":true,"format":"markdown","path":path,"report_sequence":sequence.to_string(),
            "compute_sequence":key.compute_sequence.to_string(),"report_digest":archive.report_digest,
            "archive_digest":archive.archive_digest,"bytes":archive.markdown.len().to_string(),
            "engineering_review_required":true}),
        )
    }
}
