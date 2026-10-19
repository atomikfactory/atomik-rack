//! The per-thread worker guard counts allocations on forbidden threads (support).
//! Separate binary: `WORKER_VIOLATIONS` is process-global.
atomik_testkit::install_alloc_guard!();

use atomik_testkit::no_alloc::{forbid_on_this_thread, worker_violations};

#[test]
fn forbidden_thread_allocation_is_counted() {
    assert_eq!(worker_violations(), 0);
    std::thread::spawn(|| {
        forbid_on_this_thread();
        std::hint::black_box(vec![1u8; 32]);
    })
    .join()
    .unwrap();
    // One allocation plus one deallocation, at least.
    assert!(worker_violations() >= 2, "got {}", worker_violations());
}
