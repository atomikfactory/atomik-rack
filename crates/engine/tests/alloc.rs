//! The render path never allocates, and the guard provably bites.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::NoteKind;
use atomik_engine::{DeviceInstanceId, EngineConfig, Graph, RenderPlan, ScheduledNote, new_engine};
use atomik_testkit::fakes::{
    Allocator, CallLog, ConstantSource, ImpulseOnNote, NoteThru, Passthrough, RecordingSink,
};
use common::*;

/// One fixture of every kind: interface routing, a chain, fan-out, notes through a cable.
/// Returns the plan and the ids of (constant source, note thru, impulse).
fn kitchen_sink(c: &EngineConfig) -> (RenderPlan, [DeviceInstanceId; 3]) {
    let mut g = Graph::new(c);
    let k = g.add_device(ConstantSource::boxed(0.1));
    let p = g.add_device(Passthrough::boxed(1.5));
    let thru = g.add_device(NoteThru::boxed());
    let imp = g.add_device(ImpulseOnNote::boxed());
    let (log, _) = CallLog::boxed();
    let l = g.add_device(log);
    let (sink, _) = RecordingSink::boxed(64);
    let s = g.add_device(sink);
    g.connect(iface(&g, "in_1"), ep(p, Passthrough::AUDIO_IN))
        .unwrap();
    g.connect(ep(p, Passthrough::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    g.connect(ep(k, ConstantSource::AUDIO_OUT), ep(l, CallLog::AUDIO_IN))
        .unwrap();
    g.connect(
        ep(k, ConstantSource::AUDIO_OUT),
        ep(s, RecordingSink::AUDIO_IN),
    )
    .unwrap();
    g.connect(ep(l, CallLog::AUDIO_OUT), iface(&g, "out_2"))
        .unwrap();
    g.connect(
        ep(thru, NoteThru::NOTES_OUT),
        ep(imp, ImpulseOnNote::NOTES_IN),
    )
    .unwrap();
    (g.compile().unwrap(), [k, thru, imp])
}

#[test]
fn render_is_allocation_free() {
    // render (under the guard, see `render_block`) while params, notes and plan swaps
    // flow on every host block, across all the block sizes.
    let c = cfg(1, 2);
    let (mut engine, mut handle) = new_engine(c);
    let mut frame = 0u64;
    for (i, block) in [1usize, 7, 63, 64, 65, 100, 512, 4096]
        .into_iter()
        .enumerate()
    {
        let (plan, [k, thru, imp]) = kitchen_sink(&c);
        handle.submit_plan(plan).unwrap();
        handle
            .set_param(k, ConstantSource::VALUE, i as f32 * 0.1)
            .unwrap();
        for (device, socket) in [(thru, NoteThru::NOTES_IN), (imp, ImpulseOnNote::NOTES_IN)] {
            handle
                .send_note(ScheduledNote {
                    frame: frame + 3,
                    device,
                    socket,
                    kind: NoteKind::On {
                        key: 1,
                        velocity: 1.0,
                    },
                })
                .unwrap();
        }
        let ins = vec![vec![0.25; block]];
        render_block(&mut engine, &ins, 2, block);
        render_block(&mut engine, &ins, 2, block);
        frame += 2 * block as u64;
        handle.collect_garbage();
    }
    assert_eq!(handle.dropped_events(), 0);
}

#[test]
#[should_panic(expected = "HI-1 violated")]
fn allocating_device_is_caught_by_guard() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    g.add_device(Allocator::boxed());
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    render_block(&mut engine, &[], 1, 64);
}
