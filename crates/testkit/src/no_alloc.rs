//! Allocation guard proving HI-1 (no allocation on the render path).
//!
//! Each test binary that uses the guard must install the counting allocator once:
//!
//! ```ignore
//! atomik_testkit::install_alloc_guard!();
//! ```
//!
//! [`assert_no_alloc`] then panics (in normal test code, not inside the allocator) if the
//! closure allocated, reallocated or freed heap memory on the current thread.
//!
//! [`peak_heap_of`] measures the most heap bytes a closure held at once on the current thread
//! (for tests that bound the memory a hostile input can make a reader take).
//!
//! **Worker threads.** Threads the test does not drive directly (engine
//! workers) cannot be wrapped in a closure. [`forbid_on_this_thread`] (an `fn()`, suitable for
//! `WorkerConfig::on_thread_start`) marks the calling thread as forbidden for the rest of its
//! life: every heap operation on it increments a process-global counter read with
//! [`worker_violations`]. The counter is global, so a test binary that expects 0 must not also
//! host a test that deliberately allocates on a forbidden thread.
#![allow(unsafe_code)] // Justification: implementing `GlobalAlloc` is inherently unsafe; this
// module only forwards to `System` and updates thread-local counters. Test-only crate.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

thread_local! {
    static FORBID_DEPTH: Cell<u32> = const { Cell::new(0) };
    static VIOLATIONS: Cell<u64> = const { Cell::new(0) };
    static WORKER_FORBIDDEN: Cell<bool> = const { Cell::new(false) };
    static LIVE_BYTES: Cell<isize> = const { Cell::new(0) };
    static PEAK_BYTES: Cell<isize> = const { Cell::new(0) };
}

static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Heap operations performed on threads marked by [`forbid_on_this_thread`], process-wide.
static WORKER_VIOLATIONS: AtomicU64 = AtomicU64::new(0);

/// Global allocator that counts heap operations performed inside [`assert_no_alloc`] sections.
pub struct GuardAllocator;

/// Adds `delta` to this thread's live heap bytes and remembers the highest value.
#[inline]
fn track(delta: isize) {
    let _ = LIVE_BYTES.try_with(|live| {
        live.set(live.get() + delta);
        let _ = PEAK_BYTES.try_with(|peak| peak.set(peak.get().max(live.get())));
    });
}

#[inline]
fn record() {
    INSTALLED.store(true, Ordering::Relaxed);
    let _ = FORBID_DEPTH.try_with(|depth| {
        if depth.get() > 0 {
            let _ = VIOLATIONS.try_with(|v| v.set(v.get() + 1));
        }
    });
    if WORKER_FORBIDDEN.try_with(Cell::get).unwrap_or(false) {
        WORKER_VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: every method forwards unchanged to `System`, which upholds the `GlobalAlloc` contract;
// the extra work only touches const-initialised thread-locals that never allocate.
unsafe impl GlobalAlloc for GuardAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: forwarded with the caller's layout.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            track(layout.size() as isize);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: forwarded with the caller's layout.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            track(layout.size() as isize);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record();
        track(-(layout.size() as isize));
        // SAFETY: `ptr` was allocated by `System` through this allocator with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record();
        // SAFETY: `ptr`/`layout` come from this allocator; `new_size` is the caller's request.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            track(new_size as isize - layout.size() as isize);
        }
        p
    }
}

/// Installs [`GuardAllocator`] as the global allocator of the current test binary.
#[macro_export]
macro_rules! install_alloc_guard {
    () => {
        #[global_allocator]
        static ATOMIK_ALLOC_GUARD: $crate::no_alloc::GuardAllocator =
            $crate::no_alloc::GuardAllocator;
    };
}

/// Runs `f` and panics if it performed any heap allocation, reallocation or deallocation
/// on this thread. Panics if [`install_alloc_guard!`] was not used in this test binary.
pub fn assert_no_alloc<R>(f: impl FnOnce() -> R) -> R {
    drop(std::hint::black_box(Box::new(0u8))); // marks the guard as installed if it is
    assert!(
        INSTALLED.load(Ordering::Relaxed),
        "atomik_testkit::install_alloc_guard!() is missing from this test binary"
    );
    VIOLATIONS.with(|v| v.set(0));
    FORBID_DEPTH.with(|d| d.set(d.get() + 1));
    let result = f();
    FORBID_DEPTH.with(|d| d.set(d.get() - 1));
    let violations = VIOLATIONS.with(|v| v.replace(0));
    assert!(
        violations == 0,
        "HI-1 violated: {violations} heap operation(s) inside a no-alloc section"
    );
    result
}

/// Runs `f` and returns its result with the most heap bytes it held at once on this thread
/// (memory other threads allocate is not counted). Panics if [`install_alloc_guard!`] was not
/// used in this test binary.
pub fn peak_heap_of<R>(f: impl FnOnce() -> R) -> (R, usize) {
    drop(std::hint::black_box(Box::new(0u8))); // marks the guard as installed if it is
    assert!(
        INSTALLED.load(Ordering::Relaxed),
        "atomik_testkit::install_alloc_guard!() is missing from this test binary"
    );
    let base = LIVE_BYTES.with(Cell::get);
    PEAK_BYTES.with(|p| p.set(base));
    let result = f();
    let peak = PEAK_BYTES.with(Cell::get) - base;
    (result, usize::try_from(peak).unwrap_or(0))
}

/// Marks the calling thread as allocation-forbidden for the rest of its life. Every
/// later heap operation on it increments the global [`worker_violations`] counter (it does not
/// panic: worker threads have nobody to report a panic to). Pass it as
/// `WorkerConfig::on_thread_start`.
///
/// # Panics
/// If [`install_alloc_guard!`] was not used in this test binary (a guard that cannot see
/// allocations would make every check pass vacuously).
pub fn forbid_on_this_thread() {
    drop(std::hint::black_box(Box::new(0u8))); // marks the guard as installed if it is
    assert!(
        INSTALLED.load(Ordering::Relaxed),
        "atomik_testkit::install_alloc_guard!() is missing from this test binary"
    );
    WORKER_FORBIDDEN.with(|f| f.set(true));
}

/// Heap operations seen so far on every thread marked by [`forbid_on_this_thread`].
///
/// Read it while the forbidden threads are still running (for engine workers: before dropping
/// the engine). Once a thread's code has returned, `std` frees the boxed thread closure on
/// that thread, which counts as heap operations on that thread (how many is platform-specific).
pub fn worker_violations() -> u64 {
    WORKER_VIOLATIONS.load(Ordering::Acquire)
}
