//! Passive owner-poll ENTRY witness, enabled only with native-boundary-harness.
//! A process-static counter survives App/open/session recreation. No reset API.
//! Classification concerns ordering only, not ABA, stale state or acceptance.
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const COUNTER_SCOPE: &str = "circular-owner-poll-entry-process-v1";

struct PollSerial(AtomicU64);
impl PollSerial {
    const fn new() -> Self { Self(AtomicU64::new(0)) }
    fn enter(&self) {
        // MAX is reserved UNKNOWN. checked_add fails there and leaves it latched.
        let _ = self.0.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_add(1));
    }
    fn snapshot(&self) -> Option<u64> {
        let value = self.0.load(Ordering::SeqCst);
        (value != u64::MAX).then_some(value)
    }
}
static OWNER_POLL_SERIAL: PollSerial = PollSerial::new();
pub(crate) fn enter_poll() { OWNER_POLL_SERIAL.enter(); }
pub(crate) fn snapshot() -> Option<u64> { OWNER_POLL_SERIAL.snapshot() }

// A trace consumer must decode canonical decimal strings without f64 and join
// the envelope to Root-verified PID/birth/EXE identity. Writer sequence/time is
// not a poll witness. Missing/invalid fields are not zero/default identities.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity<'a> {
    pub counter_scope: &'a str,
    pub run_id: &'a str,
    pub process_incarnation: &'a str,
    pub api_session_id: &'a str,
    pub document_uuid: &'a str,
    pub runtime_instance_id: &'a str,
    pub open_generation: u64,
}
impl Identity<'_> {
    fn valid(self) -> bool {
        [self.run_id, self.process_incarnation, self.api_session_id,
         self.document_uuid, self.runtime_instance_id].iter().all(|s| !s.is_empty())
            && self.open_generation != 0 && self.counter_scope == COUNTER_SCOPE
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Capture<'a> {
    pub identity: Identity<'a>,
    pub revision: u64,
    pub poll_serial: Option<u64>,
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct HandlerTrace<'a> {
    pub request_id: &'a str,
    pub method: &'a str,
    pub ok: bool,
    pub before: Capture<'a>,
    pub after: Capture<'a>,
}
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unknown {
    UnboundProcess, MissingOrAmbiguousTrace, WrongRequests, FailedRequest,
    Identity, RevisionSequence, CounterUnavailable, CounterRegressed, InsideHandlerPoll,
}
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PollOrder {
    NoOwnerPollEntryBetweenHandlers,
    OwnerPollEnteredBetweenHandlers,
    Unknown(Unknown),
}
/// Caller supplies ALL matching handler records, never selects the first match.
/// This does not prove an ABA edit, hook execution, stale state or poll completion.
#[allow(dead_code)]
pub(crate) fn classify(
    edits: &[HandlerTrace<'_>], undos: &[HandlerTrace<'_>], process_binding_verified: bool,
) -> PollOrder {
    use PollOrder::*;
    if !process_binding_verified { return Unknown(self::Unknown::UnboundProcess); }
    let ([edit], [undo]) = (edits, undos) else {
        return Unknown(self::Unknown::MissingOrAmbiguousTrace);
    };
    if edit.request_id.is_empty() || undo.request_id.is_empty()
        || edit.request_id == undo.request_id
        || edit.request_id == "request_id_omitted_over_size_or_complex"
        || undo.request_id == "request_id_omitted_over_size_or_complex"
        || edit.method != "geotech.ground.update" || undo.method != "document.undo"
    { return Unknown(self::Unknown::WrongRequests); }
    if !edit.ok || !undo.ok { return Unknown(self::Unknown::FailedRequest); }
    let id = edit.before.identity;
    if !id.valid() || [edit.after, undo.before, undo.after].iter().any(|c| c.identity != id) {
        return Unknown(self::Unknown::Identity);
    }
    if edit.before.revision.checked_add(1) != Some(edit.after.revision)
        || undo.before.revision != edit.after.revision
        || undo.before.revision.checked_add(1) != Some(undo.after.revision)
    { return Unknown(self::Unknown::RevisionSequence); }
    let (Some(eb), Some(ea), Some(ub), Some(ua)) =
        (edit.before.poll_serial, edit.after.poll_serial, undo.before.poll_serial, undo.after.poll_serial)
    else { return Unknown(self::Unknown::CounterUnavailable); };
    // MAX is not a known serial even if a malformed decoder supplies Some(MAX).
    if [eb, ea, ub, ua].contains(&u64::MAX) { return Unknown(self::Unknown::CounterUnavailable); }
    if eb > ea || ea > ub || ub > ua { return Unknown(self::Unknown::CounterRegressed); }
    if eb != ea || ub != ua { return Unknown(self::Unknown::InsideHandlerPoll); }
    if ea == ub { NoOwnerPollEntryBetweenHandlers } else { OwnerPollEnteredBetweenHandlers }
}

pub(crate) fn after_reply_gate_method_allowed(method: &str) -> bool {
    matches!(method, "command.apply" | "geotech.ground.update")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> (HandlerTrace<'static>, HandlerTrace<'static>) {
        let id = Identity { counter_scope: COUNTER_SCOPE, run_id: "run-06", process_incarnation: "root-verified-pid-birth-exe",
            api_session_id: "session", document_uuid: "document", runtime_instance_id: "runtime", open_generation: 3 };
        let c = |revision| Capture { identity: id, revision, poll_serial: Some(7) };
        let r = (1_u64 << 53) + 3;
        (HandlerTrace { request_id: "edit-id", method: "geotech.ground.update", ok: true, before: c(r), after: c(r+1) },
         HandlerTrace { request_id: "undo-id", method: "document.undo", ok: true, before: c(r+1), after: c(r+2) })
    }
    #[test] fn known_counter_increments_and_reads_do_not_tick() {
        let c=PollSerial::new(); assert_eq!(c.snapshot(),Some(0)); assert_eq!(c.snapshot(),Some(0));
        c.enter(); assert_eq!(c.snapshot(),Some(1)); c.enter(); assert_eq!(c.snapshot(),Some(2));
    }
    #[test] fn boundary_latches_unknown_forever_instead_of_equal_known_max() {
        let c=PollSerial(AtomicU64::new(u64::MAX-2)); c.enter(); assert_eq!(c.snapshot(),Some(u64::MAX-1));
        c.enter(); assert_eq!(c.snapshot(),None); for _ in 0..4 {c.enter(); assert_eq!(c.snapshot(),None);}
        let (mut e,mut u)=pair(); e.before.poll_serial=None; e.after.poll_serial=None;
        u.before.poll_serial=None; u.after.poll_serial=None;
        assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::CounterUnavailable));
    }
    #[test] fn equal_known_serials_classify_only_the_gap_above_f64_integer_limit() {
        let (e,u)=pair(); assert_eq!(classify(&[e],&[u],true),PollOrder::NoOwnerPollEntryBetweenHandlers);
    }
    #[test] fn growth_in_gap_is_interleaving_not_no_poll() {
        let (e,mut u)=pair(); u.before.poll_serial=Some(9); u.after.poll_serial=Some(9);
        assert_eq!(classify(&[e],&[u],true),PollOrder::OwnerPollEnteredBetweenHandlers);
    }
    #[test] fn growth_inside_a_handler_cannot_locate_the_mutation_boundary() {
        let (mut e,mut u)=pair(); e.after.poll_serial=Some(8); u.before.poll_serial=Some(8); u.after.poll_serial=Some(8);
        assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::InsideHandlerPoll));
    }
    #[test] fn missing_ambiguous_unbound_or_failed_records_never_prove_order() {
        let (e,mut u)=pair();
        assert_eq!(classify(&[],&[u],true),PollOrder::Unknown(Unknown::MissingOrAmbiguousTrace));
        assert_eq!(classify(&[e,e],&[u],true),PollOrder::Unknown(Unknown::MissingOrAmbiguousTrace));
        assert_eq!(classify(&[e],&[u],false),PollOrder::Unknown(Unknown::UnboundProcess));
        u.ok=false; assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::FailedRequest));
    }
    #[test] fn changed_identity_or_counter_regression_is_unknown() {
        let (e,mut u)=pair(); u.before.identity.open_generation+=1;
        assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::Identity));
        let (e,mut u)=pair(); u.before.poll_serial=Some(6);u.after.poll_serial=Some(6);
        assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::CounterRegressed));
    }
    #[test]
    fn after_reply_gate_method_whitelist_is_exact() {
        assert!(after_reply_gate_method_allowed("command.apply"));
        assert!(after_reply_gate_method_allowed("geotech.ground.update"));
        for method in [
            "document.undo",
            "context",
            "context.get",
            "job_status",
            "job.status",
            "geotech.ground.create",
            "geotech.ground.update.extra",
            "xgeotech.ground.update",
            "Geotech.ground.update",
            "geotech.ground.Update",
            " command.apply",
            "command.apply ",
            " geotech.ground.update",
            "geotech.ground.update ",
        ] {
            assert!(!after_reply_gate_method_allowed(method), "{method:?}");
        }
    }
    #[test] fn wrong_ids_methods_revision_gaps_and_overflow_are_unknown() {
        let (e,mut u)=pair(); u.request_id=e.request_id;
        assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::WrongRequests));
        let (e,mut u)=pair(); u.method="document.redo";
        assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::WrongRequests));
        let (e,mut u)=pair(); u.after.revision+=1;
        assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::RevisionSequence));
        let (mut e,u)=pair(); e.before.revision=u64::MAX; e.after.revision=0;
        assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::RevisionSequence));
        let (mut e,u)=pair(); e.before.poll_serial=Some(u64::MAX);
        assert_eq!(classify(&[e],&[u],true),PollOrder::Unknown(Unknown::CounterUnavailable));
    }
}
