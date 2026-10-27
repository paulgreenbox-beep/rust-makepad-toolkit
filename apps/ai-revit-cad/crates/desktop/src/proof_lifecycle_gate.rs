//! Consumer-only admission/close intent. No OS input, socket, thread or process.
//! Compile as a module in the isolated Linux consumer; never edit Makepad.
//! The App owns authentication, procfs/SHA observation, worker shutdown and the
//! single actual CxOsOp::CloseWindow. This gate does not claim a numeric XID.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowKey {
    pub index: usize,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub profile_id: String,
    pub run_id: String,
    pub session_id: String,
    pub owner: String,
    pub pid: u32,
    pub start_ticks: u64,
    pub pid_namespace: String,
    pub elf_sha256: String,
    pub window: WindowKey,
}

/// Values must come from the App's actual main_window and Cx on the UI thread,
/// not request parameters, a title, X11 inventory or a supplied native handle.
#[derive(Clone, Copy, Debug)]
pub struct NativeObservation {
    pub main: WindowKey,
    pub pool_slots: usize,
    pub valid_generation: bool,
    pub is_created: bool,
    pub is_popup: bool,
    pub kind_id: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    InvalidBinding,
    Unauthorized,
    IdentityMismatch,
    NativeWindowNotReady,
    MultipleWindowSlots,
    DeadlineExpired,
    InvalidCommandId,
    DifferentCloseCommand,
    PrepareRequired,
    WorkerNotReaped,
    AlreadyCommitted,
    SessionClosed,
}

impl Failure {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidBinding => "LIFECYCLE_BINDING_INVALID",
            Self::Unauthorized => "LIFECYCLE_AUTH_REQUIRED",
            Self::IdentityMismatch => "LIFECYCLE_IDENTITY_MISMATCH",
            Self::NativeWindowNotReady => "LIFECYCLE_NATIVE_WINDOW_NOT_READY",
            Self::MultipleWindowSlots => "LIFECYCLE_SINGLE_WINDOW_REQUIRED",
            Self::DeadlineExpired => "LIFECYCLE_DEADLINE_EXPIRED",
            Self::InvalidCommandId => "LIFECYCLE_COMMAND_ID_INVALID",
            Self::DifferentCloseCommand => "LIFECYCLE_DIFFERENT_CLOSE_COMMAND",
            Self::PrepareRequired => "LIFECYCLE_PREPARE_REQUIRED",
            Self::WorkerNotReaped => "LIFECYCLE_WORKER_NOT_REAPED",
            Self::AlreadyCommitted => "LIFECYCLE_CLOSE_ALREADY_COMMITTED",
            Self::SessionClosed => "LIFECYCLE_SESSION_CLOSED",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Running,
    Prepared { command_id: String },
    Committed,
    Closed,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CloseIntent {
    pub window: WindowKey,
}

pub struct LifecycleGate {
    binding: Binding,
    deadline_ms: u64,
    stage: Stage,
}

fn token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

impl LifecycleGate {
    pub fn new(binding: Binding, now_ms: u64, budget_ms: u64) -> Result<Self, Failure> {
        let ns = binding
            .pid_namespace
            .strip_prefix("pid:[")
            .and_then(|s| s.strip_suffix(']'));
        if binding.profile_id != "ai-revit-source03-linux-gui-v1"
            || ![&binding.run_id, &binding.session_id, &binding.owner]
                .into_iter()
                .all(|s| token(s))
            || binding.pid == 0
            || binding.start_ticks == 0
            || !ns.is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            || binding.elf_sha256.len() != 64
            || !binding
                .elf_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || budget_ms == 0
            || budget_ms > 90_000
        {
            return Err(Failure::InvalidBinding);
        }
        let deadline_ms = now_ms
            .checked_add(budget_ms)
            .ok_or(Failure::InvalidBinding)?;
        Ok(Self {
            binding,
            deadline_ms,
            stage: Stage::Running,
        })
    }

    pub fn stage(&self) -> &Stage {
        &self.stage
    }
    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    fn check(
        &self,
        authenticated: bool,
        observed: &Binding,
        native: NativeObservation,
    ) -> Result<(), Failure> {
        if !authenticated {
            return Err(Failure::Unauthorized);
        }
        if observed != &self.binding || native.main != self.binding.window {
            return Err(Failure::IdentityMismatch);
        }
        // Pool slots include stale entries: conservatively refuse those too.
        if native.pool_slots != 1 {
            return Err(Failure::MultipleWindowSlots);
        }
        if !native.valid_generation
            || !native.is_created
            || native.is_popup
            || native.kind_id != 0
            || native.main.index != 0
        {
            return Err(Failure::NativeWindowNotReady);
        }
        Ok(())
    }

    /// Authenticated observation; deliberately does not claim XID/presentation.
    /// May be queried after deadline to reconcile an unknown close outcome.
    pub fn status(
        &self,
        authenticated: bool,
        observed: &Binding,
        native: NativeObservation,
    ) -> Result<&Stage, Failure> {
        self.check(authenticated, observed, native)?;
        Ok(&self.stage)
    }

    /// Returns true only once: App must begin existing worker shutdown then.
    /// Prepare NEVER emits CloseWindow, even if the worker is already quiescent.
    pub fn prepare(
        &mut self,
        authenticated: bool,
        observed: &Binding,
        native: NativeObservation,
        command_id: &str,
        now_ms: u64,
    ) -> Result<bool, Failure> {
        self.check(authenticated, observed, native)?;
        if now_ms >= self.deadline_ms {
            return Err(Failure::DeadlineExpired);
        }
        if !token(command_id) {
            return Err(Failure::InvalidCommandId);
        }
        match &self.stage {
            Stage::Running => {
                self.stage = Stage::Prepared {
                    command_id: command_id.to_owned(),
                };
                Ok(true)
            }
            Stage::Prepared { command_id: prior } if prior == command_id => Ok(false),
            Stage::Prepared { .. } => Err(Failure::DifferentCloseCommand),
            Stage::Committed => Err(Failure::AlreadyCommitted),
            Stage::Closed => Err(Failure::SessionClosed),
        }
    }

    /// One immutable target, one close intent. Only the App applies it, AFTER
    /// its API polling loop. Commit reply delivery is not guaranteed/required;
    /// completion is independently observed process exit 0, never ACK alone.
    pub fn commit(
        &mut self,
        authenticated: bool,
        observed: &Binding,
        native: NativeObservation,
        command_id: &str,
        now_ms: u64,
        worker_reaped: bool,
    ) -> Result<CloseIntent, Failure> {
        self.check(authenticated, observed, native)?;
        if now_ms >= self.deadline_ms {
            return Err(Failure::DeadlineExpired);
        }
        if !token(command_id) {
            return Err(Failure::InvalidCommandId);
        }
        match &self.stage {
            Stage::Running => return Err(Failure::PrepareRequired),
            Stage::Prepared { command_id: prior } if prior != command_id => {
                return Err(Failure::DifferentCloseCommand);
            }
            Stage::Prepared { .. } => {}
            Stage::Committed => return Err(Failure::AlreadyCommitted),
            Stage::Closed => return Err(Failure::SessionClosed),
        }
        if !worker_reaped {
            return Err(Failure::WorkerNotReaped);
        }
        self.stage = Stage::Committed;
        Ok(CloseIntent {
            window: self.binding.window,
        })
    }

    /// This event is emitted before XDestroyWindow in the current backend.
    /// Record it but do NOT call it independent XID destruction/exit proof.
    pub fn window_closed_event(&mut self, window: WindowKey) -> Result<(), Failure> {
        if window != self.binding.window {
            return Err(Failure::IdentityMismatch);
        }
        if self.stage != Stage::Committed {
            return Err(Failure::PrepareRequired);
        }
        self.stage = Stage::Closed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (LifecycleGate, Binding, NativeObservation) {
        let window = WindowKey {
            index: 0,
            generation: 7,
        };
        let b = Binding {
            profile_id: "ai-revit-source03-linux-gui-v1".into(),
            run_id: "r1".into(),
            session_id: "s1".into(),
            owner: "operator".into(),
            pid: 123,
            start_ticks: 456,
            pid_namespace: "pid:[1234]".into(),
            elf_sha256: "a".repeat(64),
            window,
        };
        let n = NativeObservation {
            main: window,
            pool_slots: 1,
            valid_generation: true,
            is_created: true,
            is_popup: false,
            kind_id: 0,
        };
        (LifecycleGate::new(b.clone(), 0, 90_000).unwrap(), b, n)
    }
    #[test]
    fn prepare_ack_cannot_close() {
        let (mut g, b, n) = fixture();
        assert_eq!(g.prepare(true, &b, n, "c1", 1), Ok(true));
        assert!(matches!(g.stage(), Stage::Prepared { .. }));
        assert_eq!(g.prepare(true, &b, n, "c1", 2), Ok(false));
    }
    #[test]
    fn commit_needs_prepare() {
        let (mut g, b, n) = fixture();
        assert_eq!(
            g.commit(true, &b, n, "c1", 1, true),
            Err(Failure::PrepareRequired)
        );
    }
    #[test]
    fn worker_must_be_actually_reaped() {
        let (mut g, b, n) = fixture();
        g.prepare(true, &b, n, "c1", 1).unwrap();
        assert_eq!(
            g.commit(true, &b, n, "c1", 2, false),
            Err(Failure::WorkerNotReaped)
        );
    }
    #[test]
    fn close_exact_owned_once() {
        let (mut g, b, n) = fixture();
        g.prepare(true, &b, n, "c1", 1).unwrap();
        assert_eq!(
            g.commit(true, &b, n, "c1", 2, true).unwrap().window,
            b.window
        );
        assert_eq!(
            g.commit(true, &b, n, "c1", 3, true),
            Err(Failure::AlreadyCommitted)
        );
        g.window_closed_event(b.window).unwrap();
        assert_eq!(g.stage(), &Stage::Closed);
    }
    #[test]
    fn authentication_before_mutation() {
        let (mut g, b, n) = fixture();
        assert_eq!(g.prepare(false, &b, n, "c1", 1), Err(Failure::Unauthorized));
        assert_eq!(g.stage(), &Stage::Running);
    }
    #[test]
    fn stale_birth_pid_elf_namespace_and_session_refused() {
        let (mut g, b, n) = fixture();
        for i in 0..6 {
            let mut x = b.clone();
            match i {
                0 => x.pid += 1,
                1 => x.start_ticks += 1,
                2 => x.elf_sha256 = "b".repeat(64),
                3 => x.pid_namespace = "pid:[9]".into(),
                4 => x.session_id = "other".into(),
                _ => x.owner = "other".into(),
            };
            assert_eq!(
                g.prepare(true, &x, n, "c1", 1),
                Err(Failure::IdentityMismatch)
            );
        }
    }
    #[test]
    fn changed_window_generation_refused_before_commit() {
        let (mut g, b, mut n) = fixture();
        g.prepare(true, &b, n, "c1", 1).unwrap();
        n.main.generation += 1;
        assert_eq!(
            g.commit(true, &b, n, "c1", 2, true),
            Err(Failure::IdentityMismatch)
        );
    }
    #[test]
    fn native_not_created_popup_or_wrong_kind_refused() {
        let (mut g, b, n) = fixture();
        for i in 0..4 {
            let mut x = n;
            match i {
                0 => x.is_created = false,
                1 => x.is_popup = true,
                2 => x.kind_id = 1,
                _ => x.valid_generation = false,
            };
            assert_eq!(
                g.prepare(true, &b, x, "c1", 1),
                Err(Failure::NativeWindowNotReady)
            );
        }
    }
    #[test]
    fn ambiguous_or_recycled_pool_slots_refused() {
        let (mut g, b, mut n) = fixture();
        n.pool_slots = 2;
        assert_eq!(
            g.prepare(true, &b, n, "c1", 1),
            Err(Failure::MultipleWindowSlots)
        );
    }
    #[test]
    fn deadline_does_not_emit_close_but_status_still_available() {
        let (mut g, b, n) = fixture();
        g.prepare(true, &b, n, "c1", 1).unwrap();
        assert_eq!(
            g.commit(true, &b, n, "c1", 90_000, true),
            Err(Failure::DeadlineExpired)
        );
        assert!(g.status(true, &b, n).is_ok());
    }
    #[test]
    fn command_cannot_switch_between_prepare_and_commit() {
        let (mut g, b, n) = fixture();
        g.prepare(true, &b, n, "c1", 1).unwrap();
        assert_eq!(
            g.commit(true, &b, n, "c2", 2, true),
            Err(Failure::DifferentCloseCommand)
        );
    }
    #[test]
    fn forged_closed_event_not_accepted() {
        let (mut g, b, _) = fixture();
        assert_eq!(
            g.window_closed_event(b.window),
            Err(Failure::PrepareRequired)
        );
    }
    #[test]
    fn invalid_bootstrap_and_unbounded_deadline_refused() {
        let (_, mut b, _) = fixture();
        assert!(LifecycleGate::new(b.clone(), 0, 90_001).is_err());
        b.pid = 0;
        assert!(LifecycleGate::new(b, 0, 90_000).is_err());
    }
}
