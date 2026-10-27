//! Native report adoption. Full formatting stays on the owned formatter worker.
use super::*;
use geotech_circular_reportworker::ReportPhase;
use geotech_circular_ui::ReportTicket;
use serde::Serialize;
use std::io::{self, Write};

struct SnapshotCopyBudget(usize);
impl Write for SnapshotCopyBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| io::Error::other("snapshot copy byte budget"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn admit_snapshot_copy(value: &impl Serialize, cap: usize) -> Result<(), String> {
    serde_json::to_writer(SnapshotCopyBudget(cap), value)
        .map_err(|_| "NATIVE_SNAPSHOT_COPY_BUDGET".to_owned())
}

fn report_has_owned_result(
    sequence: u64,
    key: &geotech_circular_reportworker::ReportKey,
    saved: &circular_jobs::PublishedResult,
) -> bool {
    key.compute_sequence == sequence
        && key.binding == saved.binding
        && key.request_digest == saved.request_digest
        && key.content_digest == saved.content_digest
        && saved.worker_arithmetic.accepted
}

#[derive(Default)]
pub(super) struct CircularReportUiState {
    ticket: Option<ReportTicket>,
    sequence: u64,
    delivered: bool,
    observed_phase: Option<ReportPhase>,
}

impl App {
    /// Metadata-only enablement. The action repeats all authority checks before
    /// its one bounded copy, factory validation, and common typed commit.
    pub(super) fn circular_refresh_snapshot_action(&mut self, cx: &mut Cx) {
        let enabled = self
            .circular_ui
            .panel
            .snapshot_save_ticket()
            .is_some_and(|ticket| {
                self.circular_report_ui
                    .ticket
                    .as_ref()
                    .is_some_and(|owned| owned.same_source(&ticket))
                    && self.circular_report_ui.delivered
                    && self.circular_report_ui.sequence == self.circular_report.sequence()
                    && self.circular_report.phase() == ReportPhase::Completed
                    && self.circular_native_report_sequence(&ticket).is_ok()
            });
        self.ui
            .button(cx, ids!(gcc_snapshot_save))
            .set_enabled(cx, enabled);
    }

    pub(super) fn circular_save_owned_snapshot(
        &mut self,
        cx: &mut Cx,
        ticket: ReportTicket,
    ) -> Result<(), String> {
        #[cfg(target_os="linux")]
        self.gui_job_archive_before(cx,&ticket)?;
        self.circular_validate_report_compute();
        if self
            .circular_report_ui
            .ticket
            .as_ref()
            .is_none_or(|owned| !owned.same_source(&ticket))
            || !self.circular_report_ui.delivered
            || self.circular_report_ui.sequence != self.circular_report.sequence()
            || self.circular_report.phase() != ReportPhase::Completed
        {
            return Err("CURRENT_NATIVE_REPORT_REQUIRED".into());
        }
        let sequence = self.circular_native_report_sequence(&ticket)?;
        let expected = self.circular_binding()?;
        let ctx = self.circular_context();
        let (snapshot, archive_digest, ground_source, case_source) = {
            let doc = self.document.as_ref().ok_or("NO_DOCUMENT")?;
            let saved = self
                .circular_host
                .as_mut()
                .ok_or("HOST_NOT_AVAILABLE")?
                .current_result(doc, &ctx, sequence)
                .map_err(|_| "CURRENT_OWNED_RESULT_REQUIRED")?;
            let (key, archive) = self
                .circular_report
                .current(doc, &ctx)
                .ok_or("CURRENT_OWNED_REPORT_REQUIRED")?;
            if !report_has_owned_result(sequence.0, key, saved) {
                return Err("REPORT_COMPUTE_AUTHORITY_MISMATCH".into());
            }
            // Admission streams borrowed data before the only save copy. The
            // compute cache/display and ready report coexist; encoded limits
            // and Core's conservative transaction reservation are not RSS caps.
            admit_snapshot_copy(archive, ai_revit_core::MAX_CIRCULAR_SNAPSHOT_BYTES)?;
            let archive_digest = archive.archive_digest.clone();
            let ground_source = key.binding.ground_source_operation.0;
            let case_source = key.binding.case_source_operation.0;
            let snapshot = doc
                .prepare_circular_snapshot(ticket.identity.analysis_id, archive.clone())
                .map_err(|error| format!("SNAPSHOT_REJECTED: {error}"))?;
            (snapshot, archive_digest, ground_source, case_source)
        };
        let receipt = self.circular_native_commit_snapshot(
            cx,
            &expected,
            ticket.identity.analysis_id,
            snapshot,
        )?;
        #[cfg(target_os="linux")]
        self.gui_job_archive_committed(cx,&ticket,&receipt,&archive_digest)
            .map_err(|error|format!("SNAPSHOT_COMMITTED_REVISION_{} · {error}",receipt.revision))?;
        // Everything below is post-commit acknowledgement. Never report that a
        // failed UI refresh or receipt check rolled back a committed snapshot.
        let acknowledge = (|| -> Result<(), String> {
            let doc = self.document.as_ref().ok_or("NO_DOCUMENT_AFTER_COMMIT")?;
            let case = doc
                .circular_case(ticket.identity.analysis_id)
                .ok_or("CASE_MISSING")?;
            let ground = doc
                .ground_section(case.definition.ground_section_id)
                .ok_or("GROUND_MISSING")?;
            if receipt.replayed
                || doc.revision() != receipt.revision
                || case.source_operation_id.get() != case_source
                || ground.source_operation_id.get() != ground_source
                || case
                    .circular_snapshot
                    .as_ref()
                    .is_none_or(|snapshot| snapshot.archive().archive_digest != archive_digest)
                || doc
                    .circular_snapshot_currentness(ticket.identity.analysis_id)
                    .map_err(|error| error.to_string())?
                    != Some(geotech_report::Currentness::Current)
            {
                return Err("SNAPSHOT_POSTCOMMIT_BINDING_MISMATCH".into());
            }
            self.circular_refresh_catalog(cx)?;
            let live = self.circular_binding()?;
            self.circular_ui
                .panel
                .accept_snapshot_saved(&ticket, &live, &archive_digest)?;
            self.set_status(
                cx,
                &format!("Расчёт сохранён в документе · ревизия {}", receipt.revision),
            );
            Ok(())
        })();
        acknowledge.map_err(|error| {
            format!(
                "SNAPSHOT_COMMITTED_REVISION_{} · {error}; обновите текущий документ",
                receipt.revision
            )
        })
    }

    pub(super) fn circular_start_report(
        &mut self,
        cx: &mut Cx,
        ticket: ReportTicket,
    ) -> Result<(), String> {
        #[cfg(target_os="linux")]
        let gui_job_profile=self.gui_persistence_enabled();
        let compute_sequence = self.circular_native_report_sequence(&ticket)?;
        let ctx = self.circular_context();
        let doc = self.document.as_ref().ok_or("Нет открытого документа")?;
        let controller = self
            .circular_host
            .as_mut()
            .ok_or("GONE · расчёт не сохранён в обработчике")?;
        let status = controller
            .status(doc, &ctx, compute_sequence)
            .map_err(|e| format!("REPORT_REJECTED: {e:?}"))?;
        let saved = controller
            .current_result(doc, &ctx, compute_sequence)
            .map_err(|e| format!("REPORT_REJECTED: {e:?}"))?;
        #[cfg(target_os="linux")]
        if gui_job_profile {crate::linux_geotech_gui::job_report_before(&mut self.gui_hooks,&ticket,compute_sequence.0,saved)?;}
        let started = self
            .circular_report
            .start_from_saved(
                compute_sequence.0,
                saved,
                status.captured_project_revision.0,
            );
        #[cfg(target_os="linux")]
        if gui_job_profile {crate::linux_geotech_gui::job_report_returned(&mut self.gui_hooks,started.as_ref().ok().copied())?;}
        let sequence=started.map_err(|e|format!("REPORT_REJECTED: {e:?}"))?;
        self.circular_report_ui = CircularReportUiState {
            ticket: Some(ticket),
            sequence,
            delivered: false,
            observed_phase: None,
        };
        self.ui
            .text_input(cx, ids!(gcc_report_text))
            .set_text(cx, "Подготовка полного отчёта…");
        self.set_status(cx, "Подготовка отчёта в фоне");
        Ok(())
    }

    /// Existing owner timer calls this after host and report worker polling.
    pub(super) fn circular_publish_report(&mut self, cx: &mut Cx) {
        self.circular_refresh_snapshot_action(cx);
        if self.circular_report_ui.sequence != self.circular_report.sequence() {
            return;
        }
        let Some(ticket) = self.circular_report_ui.ticket.clone() else {
            return;
        };
        let phase = self.circular_report.phase();
        if self.circular_report_ui.delivered
            && self.circular_report_ui.observed_phase == Some(phase)
        {
            return;
        }
        if matches!(phase, ReportPhase::Idle | ReportPhase::Running) {
            return;
        }
        if phase != ReportPhase::Completed {
            self.circular_report_ui.delivered = true;
            self.circular_report_ui.observed_phase = Some(phase);
            self.ui.text_input(cx, ids!(gcc_report_text)).set_text(
                cx,
                &format!("Отчёт недоступен: {phase:?}. Новый расчёт не выполнялся."),
            );
            return;
        }
        let sequence = match self.circular_native_report_sequence(&ticket) {
            Ok(value) => value,
            Err(error) => {
                self.circular_report.cancel();
                self.circular_report_ui.delivered = true;
                self.circular_report_ui.observed_phase = Some(self.circular_report.phase());
                self.ui
                    .text_input(cx, ids!(gcc_report_text))
                    .set_text(cx, "STALE_REPORT · изменился источник результата");
                self.set_status(cx, &error);
                return;
            }
        };
        let ctx = self.circular_context();
        let Some(doc) = self.document.as_ref() else {
            return;
        };
        let Some((key, archive)) = self.circular_report.current(doc, &ctx) else {
            return;
        };
        if key.compute_sequence != sequence.0 {
            return;
        }
        // One small UTF-8 preview; the full archive remains owned by the worker.
        let mut end = archive.markdown.len().min(60 << 10);
        while !archive.markdown.is_char_boundary(end) {
            end -= 1;
        }
        let mut preview = archive.markdown[..end].to_owned();
        if end < archive.markdown.len() {
            preview.push_str("\n[Предпросмотр сокращён. Полный отчёт сохранён для экспорта.]\n");
        }
        self.circular_report_ui.delivered = true;
        self.circular_report_ui.observed_phase = Some(phase);
        if let Err(error) = self
            .circular_ui
            .panel
            .accept_report(&self.ui, cx, &ticket, &preview)
        {
            self.ui
                .text_input(cx, ids!(gcc_report_text))
                .set_text(cx, "STALE_REPORT · изменился черновик");
            self.set_status(cx, &error);
        } else {
            self.set_status(cx, "Отчёт подготовлен · требуется инженерное рассмотрение");
        }
        self.circular_refresh_snapshot_action(cx);
    }

    /// A report copy does not retain a current compute capability after the host
    /// cancels/evicts its receipt. Compare the actual borrowed saved content.
    pub(super) fn circular_validate_report_compute(&mut self) {
        if !matches!(
            self.circular_report.phase(),
            ReportPhase::Running | ReportPhase::Completed
        ) {
            return;
        }
        let Some(key) = self.circular_report.key().cloned() else {
            return;
        };
        let ctx = self.circular_context();
        let fresh = match (self.document.as_ref(), self.circular_host.as_mut()) {
            (Some(doc), Some(host)) => host
                .current_result(doc, &ctx, circular_host::DecimalU64(key.compute_sequence))
                .is_ok_and(|saved| report_has_owned_result(key.compute_sequence, &key, saved)),
            _ => false,
        };
        if !fresh {
            self.circular_report
                .invalidate_compute(key.compute_sequence);
        }
    }
    #[cfg(target_os="linux")]
    pub(crate) fn gui_job_report_readback(&mut self)->Result<Option<crate::linux_geotech_gui::Report>,String>{
        if self.circular_report.phase()!=ReportPhase::Completed||!self.circular_report.is_quiescent()
            ||!self.circular_report_ui.delivered||self.circular_report_ui.sequence!=self.circular_report.sequence(){return Ok(None);}
        let Some(ticket)=self.circular_report_ui.ticket.as_ref()else{return Ok(None);};
        let ctx=self.circular_context();let doc=self.document.as_ref().ok_or("JOB_REPORT_NO_DOCUMENT")?;
        let sequence=self.circular_report.sequence();
        let Some((key,archive))=self.circular_report.current(doc,&ctx)else{return Ok(None);};
        crate::linux_geotech_gui::job_report_metadata(ticket,key,sequence,archive).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geotech_circular_snapshot_copy_admission_uses_encoded_utf8_bytes() {
        let value = "я".repeat(8);
        // JSON strings include two quotes and UTF-8 is counted in bytes.
        assert!(admit_snapshot_copy(&value, 18).is_ok());
        assert!(admit_snapshot_copy(&value, 17).is_err());
        // Escapes also consume their actual serialized space.
        assert!(admit_snapshot_copy(&"\n", 4).is_ok());
        assert!(admit_snapshot_copy(&"\n", 3).is_err());
    }
}
