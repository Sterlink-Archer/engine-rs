//! A counting global allocator for `bench` / `fork-bench` (ARCHITECTURE §12 G1/G2, ledger
//! t091: "allocations per fork", "allocations per tick"). Owner: **P13**.
//!
//! The binary installs `#[global_allocator] static A: Counting = Counting;` (`main.rs`); the
//! counters are process-wide relaxed atomics, read as before/after deltas around the
//! measured region. Library tests run without it (the counters then stay at 0, and
//! [`installed`] says so).
//!
//! This is the one `unsafe` site of the engine workspace: `GlobalAlloc` is an `unsafe` trait.
//! It forwards every call to `System` unchanged and only bumps counters.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static FREES: AtomicU64 = AtomicU64::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// `System` plus allocation counters.
pub struct Counting;

// SAFETY: every method forwards its arguments unchanged to `System`, which upholds the
// `GlobalAlloc` contract; the counters are plain atomics and never allocate.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: forwarded unchanged (same contract as the caller's).
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREES.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        // SAFETY: forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Mark the counting allocator as installed (called once by `main`).
pub fn mark_installed() {
    INSTALLED.store(true, Ordering::Relaxed);
}
/// Whether the running binary counts allocations.
pub fn installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// A snapshot of the counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocCount {
    /// `alloc` + `alloc_zeroed` + `realloc` calls.
    pub allocs: u64,
    /// Bytes requested by those calls (a `realloc` counts its new size).
    pub bytes: u64,
    pub frees: u64,
}

impl AllocCount {
    pub fn now() -> AllocCount {
        AllocCount {
            allocs: ALLOCS.load(Ordering::Relaxed),
            bytes: BYTES.load(Ordering::Relaxed),
            frees: FREES.load(Ordering::Relaxed),
        }
    }
    /// `self - earlier`.
    pub fn since(self, earlier: AllocCount) -> AllocCount {
        AllocCount {
            allocs: self.allocs - earlier.allocs,
            bytes: self.bytes - earlier.bytes,
            frees: self.frees - earlier.frees,
        }
    }
}

/// Page faults so far (`/proc/self/stat` fields 10 `minflt` and 12 `majflt`); `None` off
/// Linux. Used instead of `getrusage` so the workspace needs no `libc` dependency.
pub fn page_faults() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Field 2 (comm) may contain spaces and parentheses: split after the last ')'.
    let rest = &s[s.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // rest[0] is field 3 (state), so field n is rest[n - 3].
    Some((f.get(7)?.parse().ok()?, f.get(9)?.parse().ok()?))
}

/// Process CPU seconds (utime + stime from `/proc/self/stat`, clock ticks of 1/100 s), so a
/// bench on a shared core reports its own CPU time next to the wall time (t066).
pub fn cpu_seconds() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = &s[s.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ut: f64 = f.get(11)?.parse().ok()?;
    let st: f64 = f.get(12)?.parse().ok()?;
    Some((ut + st) / 100.0)
}

/// Involuntary context switches of this process (`/proc/self/status`): a contention witness.
pub fn nonvoluntary_switches() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines().find_map(|l| l.strip_prefix("nonvoluntary_ctxt_switches:")).and_then(|v| v.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_faults() {
        let a = AllocCount::now();
        let b = AllocCount { allocs: a.allocs + 2, bytes: a.bytes + 10, frees: a.frees };
        assert_eq!(b.since(a), AllocCount { allocs: 2, bytes: 10, frees: 0 });
        if cfg!(target_os = "linux") {
            let (min, _maj) = page_faults().expect("/proc/self/stat");
            assert!(min > 0, "a running process has taken minor faults");
        }
    }
}
