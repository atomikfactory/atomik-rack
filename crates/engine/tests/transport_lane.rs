//! The transport lane: Play and Stop apply even while a parked retirement holds the control
//! queue, and keep their place behind the control messages queued before them otherwise.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{Graph, RETIRE_QUEUE_CAPACITY, RenderPlan, TransportCommand, new_engine};
use atomik_testkit::fakes::ConstantSource;
use common::*;

fn plan(value: f32) -> RenderPlan {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let k = g.add_device(ConstantSource::boxed(value));
    g.connect(ep(k, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    g.compile().unwrap()
}

#[test]
fn play_and_stop_apply_while_a_retirement_is_parked() {
    let (mut engine, mut handle) = new_engine(cfg(0, 1));
    // More swaps than the return queue holds, with nobody collecting: one plan is parked and
    // the swaps behind it wait.
    for _ in 0..RETIRE_QUEUE_CAPACITY + 8 {
        handle.submit_plan(plan(0.5)).unwrap();
        render_blocks(&mut engine, &[], 1, 64, 64);
    }
    assert!(handle.stalled_batches() > 0, "the control queue is held");
    handle.transport(TransportCommand::Play).unwrap();
    render_blocks(&mut engine, &[], 1, 64, 64);
    assert!(handle.transport_state().playing, "Play within one batch");
    handle.transport(TransportCommand::Stop).unwrap();
    render_blocks(&mut engine, &[], 1, 64, 64);
    assert!(!handle.transport_state().playing, "Stop within one batch");
    // Collecting lets the waiting swaps through again.
    handle.collect_garbage();
    render_blocks(&mut engine, &[], 1, 64 * 4, 64);
    let before = handle.stalled_batches();
    handle.collect_garbage();
    render_blocks(&mut engine, &[], 1, 64 * 4, 64);
    assert_eq!(handle.stalled_batches(), before, "no longer held");
}

#[test]
fn transport_keeps_its_place_behind_earlier_control_messages() {
    let (mut engine, mut handle) = new_engine(cfg(0, 1));
    handle
        .set_tempo(atomik_core::Tempo::new(120.0).unwrap())
        .unwrap();
    handle.transport(TransportCommand::Play).unwrap();
    handle.transport(TransportCommand::Stop).unwrap();
    render_blocks(&mut engine, &[], 1, 64, 64);
    assert!(!handle.transport_state().playing);
    handle.transport(TransportCommand::Play).unwrap();
    render_blocks(&mut engine, &[], 1, 64, 64);
    assert!(handle.transport_state().playing);
}
