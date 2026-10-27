//! Search-only operation accounting. Ordinary specified-circle calls have no
//! active scope and retain their existing arithmetic and admission behavior.
use super::{types::*, validation::{CResult, cancelled, error}};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, cmp::Ordering, marker::PhantomData, rc::Rc,
    sync::atomic::AtomicBool, time::{Duration, Instant}};

pub(crate) const METER_DEADLINE_REASON: &str = "Search wall-time budget exhausted";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchWorkLimits {
    pub geometry_preparations: u64,
    pub geometry_work: u64,
    pub slice_rows: u64,
    pub material_integrations: u64,
    pub bishop_function_evaluations_per_pass: u64,
    pub bishop_slice_terms: u64,
    pub contact_intervals: u64,
    pub contact_boundary_evaluations: u64,
    pub contact_material_checks: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchWorkCounters {
    pub geometry_preparations: u64,
    pub geometry_work: u64,
    pub slice_rows: u64,
    pub material_integrations: u64,
    pub bishop_function_evaluations: u64,
    pub bishop_slice_terms: u64,
    pub contact_intervals: u64,
    pub contact_boundary_evaluations: u64,
    pub contact_material_checks: u64,
    /// Actual comparator invocations, also included in geometry_work.
    pub sort_comparisons: u64,
    /// Cumulative conservative reservations, not actual comparison work.
    pub sort_work_reserved: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum WorkKind {
    GeometryPreparations, GeometryWork, SliceRows, MaterialIntegrations,
    BishopFunctionEvaluations, BishopSliceTerms, ContactIntervals,
    ContactBoundaryEvaluations, ContactMaterialChecks,
}

struct ActiveMeter {
    limits: SearchWorkLimits,
    counters: SearchWorkCounters,
    bishop_pass_evaluations: u64,
    reserved_geometry_work: u64,
    deadline: Instant,
    failure: Option<CircularError>,
}

thread_local! {
    static ACTIVE: RefCell<Option<ActiveMeter>> = const { RefCell::new(None) };
}

/// A scope belongs to its entering thread. It cannot move to another worker,
/// borrow a cancellation token in TLS, or silently nest another search scope.
pub struct SearchMeterScope { _same_thread: PhantomData<Rc<()>> }

impl SearchMeterScope {
    pub fn enter(limits: SearchWorkLimits, max_wall_time_ms: u64) -> CResult<Self> {
        if max_wall_time_ms == 0 {
            return Err(error(CircularErrorCode::InvalidParameter, "Search wall-time budget must be positive"));
        }
        let deadline = Instant::now().checked_add(Duration::from_millis(max_wall_time_ms))
            .ok_or_else(|| error(CircularErrorCode::ResourceLimit, "Search deadline is not representable"))?;
        Self::enter_until(limits, deadline)
    }

    fn enter_until(limits: SearchWorkLimits, deadline: Instant) -> CResult<Self> {
        ACTIVE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return Err(error(CircularErrorCode::InvalidParameter, "Nested search work scopes are unsupported"));
            }
            *slot = Some(ActiveMeter { limits, counters: SearchWorkCounters::default(),
                bishop_pass_evaluations: 0, reserved_geometry_work: 0, deadline, failure: None });
            Ok(Self { _same_thread: PhantomData })
        })
    }

    #[cfg(test)]
    pub(crate) fn enter_expired_for_test(limits: SearchWorkLimits) -> CResult<Self> {
        let deadline = Instant::now().checked_sub(Duration::from_secs(1))
            .expect("test deadline must be representably in the past");
        Self::enter_until(limits, deadline)
    }

    pub fn snapshot(&self) -> SearchWorkCounters { snapshot() }
}

impl Drop for SearchMeterScope {
    fn drop(&mut self) { ACTIVE.with(|slot| { *slot.borrow_mut() = None; }); }
}

pub(crate) fn snapshot() -> SearchWorkCounters {
    ACTIVE.with(|slot| slot.borrow().as_ref().map_or(SearchWorkCounters::default(), |m| m.counters))
}

fn failure(m: &mut ActiveMeter, message: impl Into<String>) -> CircularError {
    let e = error(CircularErrorCode::ResourceLimit, message);
    m.failure = Some(e.clone());
    e
}

pub(crate) fn is_meter_deadline_error(error: &CircularError) -> bool {
    error.code == CircularErrorCode::ResourceLimit && error.message == METER_DEADLINE_REASON
}

fn check_meter(m: &mut ActiveMeter) -> CResult<()> {
    if let Some(e) = &m.failure { return Err(e.clone()); }
    if Instant::now() >= m.deadline {
        return Err(failure(m, METER_DEADLINE_REASON));
    }
    Ok(())
}

pub(crate) fn check() -> CResult<()> {
    ACTIVE.with(|slot| match slot.borrow_mut().as_mut() {
        Some(m) => check_meter(m), None => Ok(()),
    })
}

pub(crate) fn charge(kind: WorkKind, amount: u64) -> CResult<()> {
    ACTIVE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(m) = slot.as_mut() else { return Ok(()); };
        check_meter(m)?;
        let (used, cap) = match kind {
            WorkKind::GeometryPreparations => (m.counters.geometry_preparations, m.limits.geometry_preparations),
            WorkKind::GeometryWork => (m.counters.geometry_work, m.limits.geometry_work),
            WorkKind::SliceRows => (m.counters.slice_rows, m.limits.slice_rows),
            WorkKind::MaterialIntegrations => (m.counters.material_integrations, m.limits.material_integrations),
            WorkKind::BishopFunctionEvaluations => (m.bishop_pass_evaluations, m.limits.bishop_function_evaluations_per_pass),
            WorkKind::BishopSliceTerms => (m.counters.bishop_slice_terms, m.limits.bishop_slice_terms),
            WorkKind::ContactIntervals => (m.counters.contact_intervals, m.limits.contact_intervals),
            WorkKind::ContactBoundaryEvaluations => (m.counters.contact_boundary_evaluations, m.limits.contact_boundary_evaluations),
            WorkKind::ContactMaterialChecks => (m.counters.contact_material_checks, m.limits.contact_material_checks),
        };
        let next = used.checked_add(amount).filter(|n| *n <= cap)
            .ok_or_else(|| failure(m, format!("Search {kind:?} work budget exhausted")))?;
        if matches!(kind, WorkKind::GeometryWork) && next.checked_add(m.reserved_geometry_work).is_none_or(|n| n > cap) {
            return Err(failure(m, "Search geometry work conflicts with reserved sort work"));
        }
        match kind {
            WorkKind::GeometryPreparations => m.counters.geometry_preparations = next,
            WorkKind::GeometryWork => m.counters.geometry_work = next,
            WorkKind::SliceRows => m.counters.slice_rows = next,
            WorkKind::MaterialIntegrations => m.counters.material_integrations = next,
            WorkKind::BishopFunctionEvaluations => {
                let total = m.counters.bishop_function_evaluations.checked_add(amount)
                    .ok_or_else(|| failure(m, "Search Bishop evaluation counter overflow"))?;
                m.bishop_pass_evaluations = next;
                m.counters.bishop_function_evaluations = total;
            }
            WorkKind::BishopSliceTerms => m.counters.bishop_slice_terms = next,
            WorkKind::ContactIntervals => m.counters.contact_intervals = next,
            WorkKind::ContactBoundaryEvaluations => m.counters.contact_boundary_evaluations = next,
            WorkKind::ContactMaterialChecks => m.counters.contact_material_checks = next,
        }
        Ok(())
    })
}

pub(crate) fn begin_bishop_pass() -> CResult<()> {
    ACTIVE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(m) = slot.as_mut() { check_meter(m)?; m.bishop_pass_evaluations = 0; }
        Ok(())
    })
}

struct SortReservation { remaining: u64, active: bool }

impl SortReservation {
    fn new(length: usize) -> CResult<Self> {
        ACTIVE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(m) = slot.as_mut() else { return Ok(Self { remaining: 0, active: false }); };
            check_meter(m)?;
            let n = u64::try_from(length).map_err(|_| failure(m, "Search sort length overflow"))?;
            let upper = n.checked_mul(n).ok_or_else(|| failure(m, "Search sort reservation overflow"))?;
            let reserved = m.reserved_geometry_work.checked_add(upper)
                .filter(|v| m.counters.geometry_work.checked_add(*v).is_some_and(|n| n <= m.limits.geometry_work))
                .ok_or_else(|| failure(m, "Search geometry work cannot reserve bounded library sort"))?;
            let cumulative = m.counters.sort_work_reserved.checked_add(upper)
                .ok_or_else(|| failure(m, "Search cumulative sort reservation overflow"))?;
            m.reserved_geometry_work = reserved;
            m.counters.sort_work_reserved = cumulative;
            Ok(Self { remaining: upper, active: true })
        })
    }
    fn comparison(&mut self) {
        if !self.active { return; }
        ACTIVE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(m) = slot.as_mut() else { return; };
            // Comparators always execute their original total order. A failed
            // reservation assertion is latched and the bounded sort finishes;
            // actual work remains visible and can never publish as accepted.
            if self.remaining == 0 {
                failure(m, "Search library sort exceeded its conservative comparison reservation");
            } else {
                self.remaining -= 1;
                m.reserved_geometry_work -= 1;
            }
            match (m.counters.geometry_work.checked_add(1), m.counters.sort_comparisons.checked_add(1)) {
                (Some(work), Some(comparisons)) => {
                    m.counters.geometry_work = work;
                    m.counters.sort_comparisons = comparisons;
                }
                _ => { failure(m, "Search actual sort comparison counter overflow"); }
            }
        });
    }
}
impl Drop for SortReservation {
    fn drop(&mut self) {
        if self.active { ACTIVE.with(|slot| {
            if let Some(m) = slot.borrow_mut().as_mut() {
                m.reserved_geometry_work = m.reserved_geometry_work.saturating_sub(self.remaining);
            }
        }); }
    }
}

/// Sorting retains the existing comparator and order. Before entering the
/// bounded standard-library sort, reserve n*n work units. Cancellation and
/// deadline latency includes completion of this one admitted library sort.
pub(crate) fn sort_by<T>(values: &mut [T], cancel: &AtomicBool,
    mut compare: impl FnMut(&T, &T) -> Ordering) -> CResult<()> {
    cancelled(cancel)?;
    let mut reservation = SortReservation::new(values.len())?;
    values.sort_by(|a, b| { reservation.comparison(); compare(a, b) });
    drop(reservation);
    cancelled(cancel)?;
    check()
}

/// Deduplication performs at most n-1 adjacent checks; reserve before entering
/// the library callback so budget errors cannot change its equivalence test.
pub(crate) fn dedup_by<T>(values: &mut Vec<T>, cancel: &AtomicBool,
    mut same: impl FnMut(&mut T, &mut T) -> bool) -> CResult<()> {
    cancelled(cancel)?;
    let mut reservation = SortReservation::new(values.len())?;
    values.dedup_by(|a, b| { reservation.comparison(); same(a, b) });
    drop(reservation);
    cancelled(cancel)?;
    check()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(cap: u64) -> SearchWorkLimits {
        SearchWorkLimits { geometry_preparations: cap, geometry_work: cap,
            slice_rows: cap, material_integrations: cap,
            bishop_function_evaluations_per_pass: cap, bishop_slice_terms: cap,
            contact_intervals: cap, contact_boundary_evaluations: cap,
            contact_material_checks: cap }
    }

    #[test]
    fn every_counter_cap_is_checked_before_charging_work() {
        for kind in [WorkKind::GeometryPreparations, WorkKind::GeometryWork,
            WorkKind::SliceRows, WorkKind::MaterialIntegrations,
            WorkKind::BishopFunctionEvaluations, WorkKind::BishopSliceTerms,
            WorkKind::ContactIntervals, WorkKind::ContactBoundaryEvaluations,
            WorkKind::ContactMaterialChecks] {
            let scope = SearchMeterScope::enter(limits(3), 60_000).unwrap();
            charge(kind, 3).unwrap();
            let before = scope.snapshot();
            assert_eq!(charge(kind, 1).unwrap_err().code, CircularErrorCode::ResourceLimit);
            assert_eq!(scope.snapshot(), before, "denied work must not be counted as performed");
            assert!(check().is_err(), "a denied budget remains terminal in this scope");
            drop(scope);
        }
    }

    #[test]
    fn bishop_function_cap_is_per_pass_and_total_count_is_cumulative() {
        let scope = SearchMeterScope::enter(limits(3), 60_000).unwrap();
        begin_bishop_pass().unwrap();
        charge(WorkKind::BishopFunctionEvaluations, 3).unwrap();
        begin_bishop_pass().unwrap();
        charge(WorkKind::BishopFunctionEvaluations, 3).unwrap();
        assert_eq!(scope.snapshot().bishop_function_evaluations, 6);
        assert!(charge(WorkKind::BishopFunctionEvaluations, 1).is_err());
        assert_eq!(scope.snapshot().bishop_function_evaluations, 6);
    }

    #[test]
    fn scopes_reject_nesting_and_drop_restores_unmetered_old_calls() {
        charge(WorkKind::GeometryWork, u64::MAX).unwrap();
        let scope = SearchMeterScope::enter(limits(3), 60_000).unwrap();
        assert!(SearchMeterScope::enter(limits(3), 60_000).is_err());
        charge(WorkKind::GeometryWork, 2).unwrap();
        assert_eq!(scope.snapshot().geometry_work, 2);
        drop(scope);
        charge(WorkKind::GeometryWork, u64::MAX).unwrap();
        let fresh = SearchMeterScope::enter(limits(3), 60_000).unwrap();
        assert_eq!(fresh.snapshot(), SearchWorkCounters::default());
    }

    #[test]
    fn scope_cleanup_also_occurs_during_unwind() {
        let unwind = std::panic::catch_unwind(|| {
            let _scope = SearchMeterScope::enter(limits(3), 60_000).unwrap();
            panic!("exercise search scope unwinding");
        });
        assert!(unwind.is_err());
        let scope = SearchMeterScope::enter(limits(3), 60_000).unwrap();
        assert_eq!(scope.snapshot(), SearchWorkCounters::default());
    }

    #[test]
    fn sort_reservation_rejects_before_comparator_or_permutation() {
        let cancel = AtomicBool::new(false);
        let scope = SearchMeterScope::enter(limits(2), 60_000).unwrap();
        let mut values = [3_u32, 1, 2];
        let calls = std::cell::Cell::new(0);
        assert!(sort_by(&mut values, &cancel, |a, b| {
            calls.set(calls.get() + 1); a.cmp(b)
        }).is_err());
        assert_eq!(values, [3, 1, 2]);
        assert_eq!(calls.get(), 0);
        assert_eq!(scope.snapshot().geometry_work, 0);
        drop(scope);
        let scope = SearchMeterScope::enter(limits(64), 60_000).unwrap();
        sort_by(&mut values, &cancel, u32::cmp).unwrap();
        assert_eq!(values, [1, 2, 3]);
        let counted = scope.snapshot();
        assert_eq!(counted.sort_work_reserved, 9);
        assert!(counted.sort_comparisons > 0 && counted.sort_comparisons <= 9);
        assert_eq!(counted.geometry_work, counted.sort_comparisons);
        charge(WorkKind::GeometryWork, 64 - counted.geometry_work).unwrap();
        assert_eq!(scope.snapshot().geometry_work, 64, "unused reservation must be released");
    }

    #[test]
    fn cancelled_sort_does_no_reserved_or_actual_work() {
        let scope = SearchMeterScope::enter(limits(64), 60_000).unwrap();
        let cancel = AtomicBool::new(true);
        let mut values = [3_u32, 1, 2];
        assert_eq!(sort_by(&mut values, &cancel, u32::cmp).unwrap_err().code,
            CircularErrorCode::Cancelled);
        assert_eq!(values, [3, 1, 2]);
        assert_eq!(scope.snapshot(), SearchWorkCounters::default());
    }
}
