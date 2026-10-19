//! A forbidden thread that only parks, unparks and spins records no violation, and threads
//! that are not forbidden are never counted (support). Separate binary because
//! `WORKER_VIOLATIONS` is process-global.
atomik_testkit::install_alloc_guard!();

use atomik_testkit::no_alloc::{forbid_on_this_thread, worker_violations};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

fn unforbidden_threads_are_not_counted() {
    std::thread::spawn(|| std::hint::black_box(vec![1u8; 32]).len())
        .join()
        .unwrap();
    std::hint::black_box(vec![1u8; 32]);
    assert_eq!(worker_violations(), 0);
}

/// One test function: the counter is process-global, so the checks must not run concurrently.
#[test]
fn worker_guard_counts_nothing_without_allocation() {
    unforbidden_threads_are_not_counted();
    park_and_unpark_do_not_allocate_on_a_forbidden_thread();
}

fn park_and_unpark_do_not_allocate_on_a_forbidden_thread() {
    let ready = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let wakes = Arc::new(AtomicU64::new(0));
    let violations_before_exit = Arc::new(AtomicU64::new(u64::MAX));
    let worker = {
        let (ready, stop, wakes, seen) = (
            Arc::clone(&ready),
            Arc::clone(&stop),
            Arc::clone(&wakes),
            Arc::clone(&violations_before_exit),
        );
        std::thread::spawn(move || {
            forbid_on_this_thread();
            ready.store(true, Ordering::Release);
            while !stop.load(Ordering::Acquire) {
                std::thread::park();
                wakes.fetch_add(1, Ordering::Relaxed);
                std::hint::spin_loop();
            }
            seen.store(worker_violations(), Ordering::Release);
        })
    };
    let thread = worker.thread().clone();
    while !ready.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    for _ in 0..200 {
        thread.unpark();
        std::thread::yield_now();
    }
    stop.store(true, Ordering::Release);
    thread.unpark();
    worker.join().unwrap();
    assert!(wakes.load(Ordering::Relaxed) > 0);
    assert_eq!(
        violations_before_exit.load(Ordering::Acquire),
        0,
        "park/unpark on a forbidden thread"
    );
    // After the thread's code returned, std frees the boxed thread closure on that thread
    // (documented on `worker_violations`): at most one heap operation per exited thread.
    assert!(
        worker_violations() <= 1,
        "teardown: {}",
        worker_violations()
    );
}
