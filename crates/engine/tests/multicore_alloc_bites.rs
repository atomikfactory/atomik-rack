//! Mutation check: a device that allocates in `process`, placed on a parallel level so a
//! worker runs it, is caught by the worker guard. Own binary: the counter is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::Graph;
use atomik_testkit::fakes::{Allocator, ConstantSource};
use atomik_testkit::no_alloc::{assert_no_alloc, forbid_on_this_thread, worker_violations};
use common::*;

#[test]
fn allocating_device_on_a_worker_is_caught_by_the_worker_guard() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    g.add_device(ConstantSource::boxed(0.0)); // level 0, item 0: the audio thread
    g.add_device(Allocator::boxed()); // level 0, item 1: the worker
    let (mut engine, mut handle) = engine_with(c, 1, Some(forbid_on_this_thread));
    handle.submit_plan(g.compile().unwrap()).unwrap();
    assert_eq!(worker_violations(), 0);
    let mut out = vec![0.0f32; 256];
    // The audio thread itself stays clean: the allocation happens on the worker.
    assert_no_alloc(|| engine.render(&[], &mut [&mut out[..]]));
    assert!(
        worker_violations() >= 4 * 2,
        "4 batches x (alloc + free), got {}",
        worker_violations()
    );
}
