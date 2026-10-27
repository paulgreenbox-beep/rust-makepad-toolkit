//! Linux-only UI-thread integration; no Xlib global, synthetic input or exit.
use super::*;
use crate::{linux_lifecycle_wire as wire, proof_lifecycle_gate as gate};
use std::{collections::BTreeSet, time::{Duration, Instant}};

#[derive(Default)]
pub(super) struct AppLifecycle {
    gate: Option<gate::LifecycleGate>, pending: Option<gate::CloseIntent>,
    binding: Option<wire::Binding>, seen_request_ids: BTreeSet<String>,
    ui_tick: u64, close_dispatched: bool,
}
fn gate_binding(value: &wire::Binding) -> gate::Binding {
    gate::Binding { profile_id: value.profile_id.clone(), run_id: value.run_id.clone(),
        session_id: value.session_id.clone(), owner: value.owner.clone(), pid: value.pid,
        start_ticks: value.start_ticks, pid_namespace: value.pid_namespace.clone(),
        elf_sha256: value.elf_sha256.clone(), window: gate::WindowKey {
            index: value.window.index, generation: value.window.generation } }
}
fn stage_name(value: &gate::Stage) -> &'static str {
    match value { gate::Stage::Running => "RUNNING", gate::Stage::Prepared { .. } => "PREPARED",
        gate::Stage::Committed => "CLOSE_COMMITTED", gate::Stage::Closed => "CLOSED_EVENT_OBSERVED" }
}
impl App {
    /// Candidate seam: no new identity implementation. The existing observer
    /// uses cheap fresh retained-lease checked_identity once per GUI request.
    pub(super) fn gui_lifecycle_observe_request(&self, cx: &Cx,
        request: &agent_api::ApiCommand, expected: &wire::Binding) -> Result<wire::Binding, String> {
        self.require_api_token(request)?;
        let frozen = self.gui_lifecycle_frozen(cx)?;
        let (observed, _) = self.lifecycle_observed(cx)?;
        wire::ensure(observed == frozen && &observed == expected, "GUI_EXPECTED_FRESH_FROZEN_MISMATCH")?;
        Ok(observed)
    }
    /// Input/post-Draw rechecks use the already frozen process binding and
    /// actual native generation; they never add a full-ELF hash pass.
    pub(super) fn gui_lifecycle_frozen(&self, cx: &Cx) -> Result<wire::Binding, String> {
        wire::ensure(wire::healthy() && self.api_receiver.is_some() && self.proof_lifecycle.ui_tick > 0,
            "GUI_LIFECYCLE_NOT_READY")?;
        wire::remaining(wire::trusted()?.deadline)?;
        let value = self.proof_lifecycle.gate.as_ref().ok_or("GUI_LIFECYCLE_NOT_READY")?;
        wire::ensure(matches!(value.stage(), gate::Stage::Running) && self.circular_close_window.is_none(),
            "GUI_LIFECYCLE_NOT_RUNNING")?;
        // Only an already-admitted ordinary Open may retain its existing
        // archive-inspection worker between members. Calculation host remains
        // quiescent, and terminal Open still requires whole-App quiescence.
        wire::ensure(self.circular_is_quiescent() || self.gui_persistence_archive_restore_window() || self.gui_job_worker_window(), "GUI_WORKER_NOT_QUIESCENT")?;
        let frozen = self.proof_lifecycle.binding.as_ref().ok_or("GUI_LIFECYCLE_NOT_READY")?;
        let (id, native) = self.lifecycle_native(cx)?;
        value.status(true, &gate_binding(frozen), native).map_err(|v| v.code().to_owned())?;
        wire::ensure(id == WindowId(0, 0) && frozen.window.index == id.0 && frozen.window.generation == id.1
            && native.pool_slots == 1 && native.is_created && !native.is_popup && native.kind_id == 0
            && frozen.pid == std::process::id(), "GUI_NATIVE_WINDOW_UNSUPPORTED")?;
        Ok(frozen.clone())
    }
    fn lifecycle_native(&self, cx: &Cx) -> Result<(WindowId, gate::NativeObservation), String> {
        let id = self.ui.window(cx, ids!(main_window)).window_id().ok_or("LIFECYCLE_NATIVE_WINDOW_NOT_READY")?;
        let valid = cx.windows.is_valid(id);
        wire::ensure(valid, "LIFECYCLE_STALE_MAIN_WINDOW")?;
        let window = &cx.windows[id];
        Ok((id, gate::NativeObservation { main: gate::WindowKey { index: id.0, generation: id.1 },
            pool_slots: cx.windows.len(), valid_generation: valid, is_created: window.is_created,
            is_popup: window.is_popup, kind_id: window.kind_id }))
    }
    fn lifecycle_observed(&self, cx: &Cx) -> Result<(wire::Binding, gate::NativeObservation), String> {
        let (_, native) = self.lifecycle_native(cx)?;
        let trusted = wire::trusted()?;
        let observed = trusted.process.checked_identity(Instant::now() + Duration::from_secs(10))?;
        let binding = trusted.bootstrap.bind(&observed, wire::WindowKey {
            index: native.main.index, generation: native.main.generation })?;
        Ok((binding, native))
    }
    pub(super) fn lifecycle_tick(&mut self, cx: &mut Cx) {
        self.proof_lifecycle.ui_tick = self.proof_lifecycle.ui_tick.saturating_add(1);
        if self.proof_lifecycle.gate.is_some() || !wire::healthy() { return; }
        if self.api_receiver.is_none() { wire::fail("LIFECYCLE_API_NOT_READY"); return; }
        let result = (|| -> Result<(), String> {
            let trusted = wire::trusted()?;
            wire::remaining(trusted.deadline)?;
            let (binding, native) = self.lifecycle_observed(cx)?;
            // is_created is set only after the backend's actual native creation.
            if !native.is_created { return Ok(()); }
            let gate = gate::LifecycleGate::new(gate_binding(&binding), 0, trusted.bootstrap.budget_ms)
                .map_err(|v| v.code().to_owned())?;
            gate.status(true, &gate_binding(&binding), native).map_err(|v| v.code().to_owned())?;
            wire::atomic_json("initial-lifecycle.json", &wire::Initial {
                schema: "source03-initial-lifecycle-v1".into(), binding: binding.clone(), state: "RUNNING".into(),
                api_ready: true, ui_tick: self.proof_lifecycle.ui_tick,
                worker_quiescent: self.circular_is_quiescent(), native_xid: None, native_xid_verified: false,
                binding_source: "app_main_window_cx_pool_generation".into() })?;
            self.proof_lifecycle.binding = Some(binding);
            self.proof_lifecycle.gate = Some(gate);
            Ok(())
        })();
        if let Err(code) = result { wire::fail(&code); }
    }
    pub(super) fn lifecycle_blocks_business(&self) -> bool {
        self.proof_lifecycle.gate.as_ref().is_some_and(|value| matches!(value.stage(),
            gate::Stage::Prepared { .. } | gate::Stage::Committed | gate::Stage::Closed))
            || !wire::healthy()
    }
    pub(super) fn dispatch_lifecycle_request(&mut self, cx: &mut Cx,
        request: &agent_api::ApiCommand) -> Result<Value, String> {
        self.require_api_token(request)?;
        wire::ensure(wire::healthy() && self.api_receiver.is_some() && self.proof_lifecycle.ui_tick > 0,
            "LIFECYCLE_RUNTIME_UNHEALTHY")?;
        let request_id = request.id.as_str().filter(|v| wire::identifier(v)).ok_or("LIFECYCLE_REQUEST_ID_INVALID")?;
        wire::ensure(self.proof_lifecycle.seen_request_ids.len() < 256 &&
            self.proof_lifecycle.seen_request_ids.insert(request_id.to_owned()), "LIFECYCLE_REQUEST_ID_REPLAY")?;
        let (observed, native) = self.lifecycle_observed(cx)?;
        let frozen = self.proof_lifecycle.binding.as_ref().ok_or("LIFECYCLE_NOT_READY")?;
        wire::ensure(&observed == frozen, "LIFECYCLE_FROZEN_BINDING_MISMATCH")?;
        wire::ensure(self.circular_close_window.is_none(), "LIFECYCLE_OTHER_CLOSE_PENDING")?;
        let quiescent = self.circular_is_quiescent();
        let elapsed = wire::trusted()?.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let mut begin_shutdown = false;
        match request.method.as_str() {
            "diagnostic.lifecycle.status" => {
                let params: wire::StatusParams = serde_json::from_value(request.params.clone())
                    .map_err(|_| "LIFECYCLE_STATUS_PARAMS_INVALID")?;
                wire::ensure(params.expected == observed, "LIFECYCLE_EXPECTED_BINDING_MISMATCH")?;
                self.proof_lifecycle.gate.as_ref().ok_or("LIFECYCLE_NOT_READY")?
                    .status(true, &gate_binding(&observed), native).map_err(|v| v.code().to_owned())?;
            }
            "diagnostic.lifecycle.close" => {
                let params: wire::CloseParams = serde_json::from_value(request.params.clone())
                    .map_err(|_| "LIFECYCLE_CLOSE_PARAMS_INVALID")?;
                wire::ensure(params.expected == observed, "LIFECYCLE_EXPECTED_BINDING_MISMATCH")?;
                let value = self.proof_lifecycle.gate.as_mut().ok_or("LIFECYCLE_NOT_READY")?;
                match params.phase {
                    wire::Phase::Prepare => {
                        begin_shutdown = value.prepare(true, &gate_binding(&observed), native,
                            &params.command_id, elapsed).map_err(|v| v.code().to_owned())?;
                    }
                    wire::Phase::Commit => {
                        wire::ensure(self.proof_lifecycle.pending.is_none() && !self.proof_lifecycle.close_dispatched,
                            "LIFECYCLE_CLOSE_ALREADY_QUEUED")?;
                        let intent = value.commit(true, &gate_binding(&observed), native,
                            &params.command_id, elapsed, quiescent).map_err(|v| v.code().to_owned())?;
                        self.proof_lifecycle.pending = Some(intent);
                    }
                }
            }
            _ => return Err("LIFECYCLE_METHOD_UNKNOWN".into()),
        }
        // Never circular_begin_close / never set circular_close_window here.
        if begin_shutdown { self.circular_begin_shutdown(); }
        let stage = self.proof_lifecycle.gate.as_ref().ok_or("LIFECYCLE_NOT_READY")?.stage();
        Ok(json!({"binding":observed, "state":stage_name(stage),
            "worker_quiescent":self.circular_is_quiescent(), "runtime_healthy":wire::healthy(),
            "ui_tick":self.proof_lifecycle.ui_tick, "native_window":{
                "window":{"index":native.main.index,"generation":native.main.generation},
                "created":native.is_created,"binding_source":"app_main_window_cx_pool_generation",
                "native_xid":null,"native_xid_verified":false}}))
    }
    /// Called only after poll_agent_api returns and existing worker polling/reaping.
    pub(super) fn lifecycle_apply_pending_close(&mut self, cx: &mut Cx) {
        if self.proof_lifecycle.pending.is_none() { return; }
        let result = (|| -> Result<(), String> {
            wire::ensure(wire::healthy() && self.circular_is_quiescent() && self.circular_close_window.is_none(),
                "LIFECYCLE_CLOSE_RECHECK_FAILED")?;
            let (observed, native) = self.lifecycle_observed(cx)?;
            wire::ensure(self.proof_lifecycle.binding.as_ref() == Some(&observed), "LIFECYCLE_CLOSE_BINDING_CHANGED")?;
            self.proof_lifecycle.gate.as_ref().ok_or("LIFECYCLE_NOT_READY")?
                .status(true, &gate_binding(&observed), native).map_err(|v| v.code().to_owned())?;
            wire::ensure(wire::trusted()?.started.elapsed().as_millis() < wire::trusted()?.bootstrap.budget_ms as u128,
                "LIFECYCLE_DEADLINE_EXPIRED")?;
            let intent = self.proof_lifecycle.pending.take().ok_or("LIFECYCLE_INTENT_MISSING")?;
            wire::ensure(intent.window == native.main, "LIFECYCLE_CLOSE_WINDOW_CHANGED")?;
            let (id, _) = self.lifecycle_native(cx)?;
            // Historical filename retained: this durable record is INTENT only.
            // fsync may exhaust the original budget; it does not prove dispatch.
            wire::atomic_json("lifecycle-close-dispatched.json", &json!({
                "schema":"source03-close-dispatched-v1","binding":observed,
                "state":"PRE_DISPATCH_INTENT","platform_op_queued_proven":false,
                "worker_quiescent":true,"ui_tick":self.proof_lifecycle.ui_tick,
                "reply_delivery_proven":false,"native_xid_verified":false}))?;
            // Final sampled gate, not a hard-realtime guarantee of OS execution.
            wire::remaining(wire::trusted()?.deadline)?;
            self.proof_lifecycle.close_dispatched = true;
            cx.push_unique_platform_op(CxOsOp::CloseWindow(id));
            Ok(())
        })();
        if let Err(code) = result { self.proof_lifecycle.pending = None; wire::fail(&code); }
    }
    pub(super) fn lifecycle_observe_closed(&mut self, id: WindowId) {
        let result = (|| -> Result<(), String> {
            wire::ensure(self.proof_lifecycle.close_dispatched, "LIFECYCLE_UNCOMMITTED_CLOSE_EVENT")?;
            self.proof_lifecycle.gate.as_mut().ok_or("LIFECYCLE_NOT_READY")?
                .window_closed_event(gate::WindowKey { index: id.0, generation: id.1 })
                .map_err(|v| v.code().to_owned())?;
            wire::atomic_json("lifecycle-window-closed.json", &json!({"schema":"source03-window-closed-event-v1",
                "window":{"index":id.0,"generation":id.1},"native_destruction_or_exit_proven":false}))?;
            wire::closed_event();
            Ok(())
        })();
        if let Err(code) = result { wire::fail(&code); }
    }
}

// Private read-only observation of the ORIGINAL trusted server Instant.
// Called last while forming Snapshot JSON; response transport time is discounted by client.
pub(super) fn gui_trusted_remaining_ms() -> Result<u64, String> {
    let left = wire::remaining(wire::trusted()?.deadline)?;
    Ok(left.as_millis().min(u64::MAX as u128) as u64)
}
