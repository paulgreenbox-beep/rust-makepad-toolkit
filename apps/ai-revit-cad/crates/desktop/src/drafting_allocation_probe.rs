//! Process-wide allocation counters for bounded runtime measurements.
//!
//! A measurement made with [`snapshot`] includes allocator calls from every
//! thread in the process. Relaxed ordering does not establish ordering with the
//! measured work, so concurrent calls may be observed on either side of a
//! snapshot. Atomic counter updates still add runtime cost and may contend
//! across threads. Bracket a synchronous render scope and account for unrelated
//! activity from other threads.
//!
//! These values describe allocator operations and byte sizes requested through
//! `GlobalAlloc`; they are not estimates of live or retained memory. In
//! particular, reallocation is reported separately instead of being counted
//! as an allocation plus a deallocation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

struct Counters {
    alloc_calls: AtomicU64,
    alloc_requested_bytes: AtomicU64,
    alloc_non_null_returns: AtomicU64,
    alloc_non_null_bytes: AtomicU64,
    alloc_null_returns: AtomicU64,
    dealloc_calls: AtomicU64,
    dealloc_layout_bytes: AtomicU64,
    realloc_calls: AtomicU64,
    realloc_old_layout_bytes: AtomicU64,
    realloc_requested_bytes: AtomicU64,
    realloc_non_null_returns: AtomicU64,
    realloc_non_null_bytes: AtomicU64,
    realloc_nonzero_null_returns: AtomicU64,
    realloc_zero_size_null_returns: AtomicU64,
}

impl Counters {
    const fn new() -> Self {
        Self {
            alloc_calls: AtomicU64::new(0),
            alloc_requested_bytes: AtomicU64::new(0),
            alloc_non_null_returns: AtomicU64::new(0),
            alloc_non_null_bytes: AtomicU64::new(0),
            alloc_null_returns: AtomicU64::new(0),
            dealloc_calls: AtomicU64::new(0),
            dealloc_layout_bytes: AtomicU64::new(0),
            realloc_calls: AtomicU64::new(0),
            realloc_old_layout_bytes: AtomicU64::new(0),
            realloc_requested_bytes: AtomicU64::new(0),
            realloc_non_null_returns: AtomicU64::new(0),
            realloc_non_null_bytes: AtomicU64::new(0),
            realloc_nonzero_null_returns: AtomicU64::new(0),
            realloc_zero_size_null_returns: AtomicU64::new(0),
        }
    }
}

static COUNTERS: Counters = Counters::new();

/// A `System` allocator wrapper that records operation and requested-byte
/// totals. Declare this type as the binary's global allocator at the call site.
/// This module deliberately does not install a global allocator itself.
pub struct CountingAllocator;

// SAFETY: Every operation is forwarded to `System` with the original pointer
// and layout. The relaxed atomic bookkeeping does not inspect or change the
// allocated memory and does not allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller of `GlobalAlloc::alloc` guarantees a valid layout.
        let pointer = unsafe { System.alloc(layout) };
        COUNTERS.alloc_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .alloc_requested_bytes
            .fetch_add(layout.size() as u64, Ordering::Relaxed);
        if pointer.is_null() {
            COUNTERS.alloc_null_returns.fetch_add(1, Ordering::Relaxed);
        } else {
            COUNTERS
                .alloc_non_null_returns
                .fetch_add(1, Ordering::Relaxed);
            COUNTERS
                .alloc_non_null_bytes
                .fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller of `GlobalAlloc::alloc_zeroed` guarantees a valid layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        COUNTERS.alloc_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .alloc_requested_bytes
            .fetch_add(layout.size() as u64, Ordering::Relaxed);
        if pointer.is_null() {
            COUNTERS.alloc_null_returns.fetch_add(1, Ordering::Relaxed);
        } else {
            COUNTERS
                .alloc_non_null_returns
                .fetch_add(1, Ordering::Relaxed);
            COUNTERS
                .alloc_non_null_bytes
                .fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: The caller must pass the pointer and layout from a compatible
        // allocation; forwarding them unchanged preserves that contract.
        unsafe { System.dealloc(pointer, layout) };
        COUNTERS.dealloc_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .dealloc_layout_bytes
            .fetch_add(layout.size() as u64, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller must pass the pointer and layout from a compatible
        // allocation; forwarding them unchanged preserves that contract.
        let result = unsafe { System.realloc(pointer, layout, new_size) };
        COUNTERS.realloc_calls.fetch_add(1, Ordering::Relaxed);
        COUNTERS
            .realloc_old_layout_bytes
            .fetch_add(layout.size() as u64, Ordering::Relaxed);
        COUNTERS
            .realloc_requested_bytes
            .fetch_add(new_size as u64, Ordering::Relaxed);
        if result.is_null() {
            if new_size == 0 {
                // A null result for a zero-sized request is allocator-defined;
                // report it separately instead of calling it a failure.
                COUNTERS
                    .realloc_zero_size_null_returns
                    .fetch_add(1, Ordering::Relaxed);
            } else {
                COUNTERS
                    .realloc_nonzero_null_returns
                    .fetch_add(1, Ordering::Relaxed);
            }
        } else {
            COUNTERS
                .realloc_non_null_returns
                .fetch_add(1, Ordering::Relaxed);
            COUNTERS
                .realloc_non_null_bytes
                .fetch_add(new_size as u64, Ordering::Relaxed);
        }
        result
    }
}

/// Cumulative process-wide totals observed from the allocator.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllocationSnapshot {
    pub alloc_calls: u64,
    pub alloc_requested_bytes: u64,
    pub alloc_non_null_returns: u64,
    pub alloc_non_null_bytes: u64,
    pub alloc_null_returns: u64,
    pub dealloc_calls: u64,
    /// Sum of the sizes in deallocation layouts supplied to the allocator.
    pub dealloc_layout_bytes: u64,
    pub realloc_calls: u64,
    /// Sum of the old layout sizes supplied to realloc calls.
    pub realloc_old_layout_bytes: u64,
    pub realloc_requested_bytes: u64,
    pub realloc_non_null_returns: u64,
    pub realloc_non_null_bytes: u64,
    /// Null returns from realloc calls with a nonzero requested size.
    pub realloc_nonzero_null_returns: u64,
    /// Kept separate because a null return for size zero is allocator-defined.
    pub realloc_zero_size_null_returns: u64,
}

/// Differences between two cumulative snapshots.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllocationDelta {
    pub alloc_calls: u64,
    pub alloc_requested_bytes: u64,
    pub alloc_non_null_returns: u64,
    pub alloc_non_null_bytes: u64,
    pub alloc_null_returns: u64,
    pub dealloc_calls: u64,
    pub dealloc_layout_bytes: u64,
    pub realloc_calls: u64,
    pub realloc_old_layout_bytes: u64,
    pub realloc_requested_bytes: u64,
    pub realloc_non_null_returns: u64,
    pub realloc_non_null_bytes: u64,
    pub realloc_nonzero_null_returns: u64,
    pub realloc_zero_size_null_returns: u64,
}

/// Capture relaxed cumulative totals without allocating.
pub fn snapshot() -> AllocationSnapshot {
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    AllocationSnapshot {
        alloc_calls: load(&COUNTERS.alloc_calls),
        alloc_requested_bytes: load(&COUNTERS.alloc_requested_bytes),
        alloc_non_null_returns: load(&COUNTERS.alloc_non_null_returns),
        alloc_non_null_bytes: load(&COUNTERS.alloc_non_null_bytes),
        alloc_null_returns: load(&COUNTERS.alloc_null_returns),
        dealloc_calls: load(&COUNTERS.dealloc_calls),
        dealloc_layout_bytes: load(&COUNTERS.dealloc_layout_bytes),
        realloc_calls: load(&COUNTERS.realloc_calls),
        realloc_old_layout_bytes: load(&COUNTERS.realloc_old_layout_bytes),
        realloc_requested_bytes: load(&COUNTERS.realloc_requested_bytes),
        realloc_non_null_returns: load(&COUNTERS.realloc_non_null_returns),
        realloc_non_null_bytes: load(&COUNTERS.realloc_non_null_bytes),
        realloc_nonzero_null_returns: load(&COUNTERS.realloc_nonzero_null_returns),
        realloc_zero_size_null_returns: load(&COUNTERS.realloc_zero_size_null_returns),
    }
}

/// Compute the cumulative-counter difference from `start` to `end`.
///
/// Counters use `u64` wrapping arithmetic, so this also handles one counter
/// rollover between snapshots. The snapshots must be chronological, and fewer
/// than `u64::MAX` events for each field must occur between them.
pub fn delta(start: AllocationSnapshot, end: AllocationSnapshot) -> AllocationDelta {
    let difference = |before: u64, after: u64| after.wrapping_sub(before);
    AllocationDelta {
        alloc_calls: difference(start.alloc_calls, end.alloc_calls),
        alloc_requested_bytes: difference(start.alloc_requested_bytes, end.alloc_requested_bytes),
        alloc_non_null_returns: difference(
            start.alloc_non_null_returns,
            end.alloc_non_null_returns,
        ),
        alloc_non_null_bytes: difference(start.alloc_non_null_bytes, end.alloc_non_null_bytes),
        alloc_null_returns: difference(start.alloc_null_returns, end.alloc_null_returns),
        dealloc_calls: difference(start.dealloc_calls, end.dealloc_calls),
        dealloc_layout_bytes: difference(start.dealloc_layout_bytes, end.dealloc_layout_bytes),
        realloc_calls: difference(start.realloc_calls, end.realloc_calls),
        realloc_old_layout_bytes: difference(
            start.realloc_old_layout_bytes,
            end.realloc_old_layout_bytes,
        ),
        realloc_requested_bytes: difference(
            start.realloc_requested_bytes,
            end.realloc_requested_bytes,
        ),
        realloc_non_null_returns: difference(
            start.realloc_non_null_returns,
            end.realloc_non_null_returns,
        ),
        realloc_non_null_bytes: difference(
            start.realloc_non_null_bytes,
            end.realloc_non_null_bytes,
        ),
        realloc_nonzero_null_returns: difference(
            start.realloc_nonzero_null_returns,
            end.realloc_nonzero_null_returns,
        ),
        realloc_zero_size_null_returns: difference(
            start.realloc_zero_size_null_returns,
            end.realloc_zero_size_null_returns,
        ),
    }
}

impl AllocationSnapshot {
    /// Serialize for reporting. Do this after the measured scope because JSON
    /// value construction itself allocates.
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "scope": "process_wide",
            "concurrency_note": "all threads; relaxed snapshots may split concurrent operations; atomic updates add runtime cost",
            "alloc_calls": self.alloc_calls,
            "alloc_requested_bytes": self.alloc_requested_bytes,
            "alloc_non_null_returns": self.alloc_non_null_returns,
            "alloc_non_null_bytes": self.alloc_non_null_bytes,
            "alloc_null_returns": self.alloc_null_returns,
            "dealloc_calls": self.dealloc_calls,
            "dealloc_layout_bytes": self.dealloc_layout_bytes,
            "realloc_calls": self.realloc_calls,
            "realloc_old_layout_bytes": self.realloc_old_layout_bytes,
            "realloc_requested_bytes": self.realloc_requested_bytes,
            "realloc_non_null_returns": self.realloc_non_null_returns,
            "realloc_non_null_bytes": self.realloc_non_null_bytes,
            "realloc_nonzero_null_returns": self.realloc_nonzero_null_returns,
            "realloc_zero_size_null_returns": self.realloc_zero_size_null_returns,
        })
    }
}

impl AllocationDelta {
    /// Serialize for reporting. Do this after the measured scope because JSON
    /// value construction itself allocates.
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "scope": "process_wide",
            "concurrency_note": "all threads; relaxed snapshots may split concurrent operations; atomic updates add runtime cost",
            "alloc_calls": self.alloc_calls,
            "alloc_requested_bytes": self.alloc_requested_bytes,
            "alloc_non_null_returns": self.alloc_non_null_returns,
            "alloc_non_null_bytes": self.alloc_non_null_bytes,
            "alloc_null_returns": self.alloc_null_returns,
            "dealloc_calls": self.dealloc_calls,
            "dealloc_layout_bytes": self.dealloc_layout_bytes,
            "realloc_calls": self.realloc_calls,
            "realloc_old_layout_bytes": self.realloc_old_layout_bytes,
            "realloc_requested_bytes": self.realloc_requested_bytes,
            "realloc_non_null_returns": self.realloc_non_null_returns,
            "realloc_non_null_bytes": self.realloc_non_null_bytes,
            "realloc_nonzero_null_returns": self.realloc_nonzero_null_returns,
            "realloc_zero_size_null_returns": self.realloc_zero_size_null_returns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AllocationDelta, AllocationSnapshot, delta};

    #[test]
    fn delta_reports_each_operation_and_byte_counter() {
        let start = AllocationSnapshot::default();
        let end = AllocationSnapshot {
            alloc_calls: 3,
            alloc_requested_bytes: 48,
            alloc_non_null_returns: 2,
            alloc_non_null_bytes: 40,
            alloc_null_returns: 1,
            dealloc_calls: 2,
            dealloc_layout_bytes: 32,
            realloc_calls: 2,
            realloc_old_layout_bytes: 24,
            realloc_requested_bytes: 64,
            realloc_non_null_returns: 1,
            realloc_non_null_bytes: 48,
            realloc_nonzero_null_returns: 1,
            realloc_zero_size_null_returns: 1,
        };
        let expected = AllocationDelta {
            alloc_calls: 3,
            alloc_requested_bytes: 48,
            alloc_non_null_returns: 2,
            alloc_non_null_bytes: 40,
            alloc_null_returns: 1,
            dealloc_calls: 2,
            dealloc_layout_bytes: 32,
            realloc_calls: 2,
            realloc_old_layout_bytes: 24,
            realloc_requested_bytes: 64,
            realloc_non_null_returns: 1,
            realloc_non_null_bytes: 48,
            realloc_nonzero_null_returns: 1,
            realloc_zero_size_null_returns: 1,
        };

        assert_eq!(delta(start, end), expected);
    }

    #[test]
    fn delta_handles_counter_rollover() {
        let start = AllocationSnapshot {
            alloc_calls: u64::MAX - 2,
            ..AllocationSnapshot::default()
        };
        let end = AllocationSnapshot {
            alloc_calls: 3,
            ..AllocationSnapshot::default()
        };

        assert_eq!(delta(start, end).alloc_calls, 6);
    }
}
