//! The audio-thread rule guard: a device that locks or blocks inside `process` is caught by a
//! guarded test as a device fault, and a well-behaved one is not.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{Graph, new_engine};
use atomik_testkit::fakes::{Locker, Misbehaviour};
use common::*;

fn fault_after_render(mode: Misbehaviour) -> bool {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    g.add_device(Locker::boxed(mode));
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    // Not `render_blocks`: a violation panics, and the panic message allocates.
    let mut out = [0.0f32; 128];
    engine.render(&[], &mut [&mut out[..]]);
    handle.fault().is_some()
}

#[test]
fn a_device_that_takes_a_lock_in_process_fails_the_guard() {
    assert!(fault_after_render(Misbehaviour::Lock));
}

#[test]
fn a_device_that_blocks_in_process_fails_the_guard() {
    assert!(fault_after_render(Misbehaviour::Block));
}

#[test]
fn a_well_behaved_device_passes_the_guard() {
    assert!(!fault_after_render(Misbehaviour::None));
}
