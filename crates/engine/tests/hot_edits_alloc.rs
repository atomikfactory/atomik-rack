//! [HI-1/HI-2]: a scripted sequence of hot edits at fixed frames (inserts, routing swaps,
//! removals, param and tempo changes) renders bit-identically twice, at host blocks
//! {1, 64, 333} and with {0, 3} workers, with no allocation on the audio thread
//! (`assert_no_alloc` around every render) or on any worker (`forbid_on_this_thread`). Own
//! binary: the worker violation counter is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_devices::{line_mixer, poly_synth, stereo_delay};
use atomik_engine::{Graph, OfflineJob, ScheduledNote, render_offline_scripted};
use atomik_testkit::no_alloc::{assert_no_alloc, forbid_on_this_thread, worker_violations};
use common::hot::{Rig, energy, note};
use common::*;

const FRAMES: usize = 8000;

/// The script: every kind of hot edit, at frames on and off batch boundaries.
fn script() -> (Rig, Vec<ScheduledNote>) {
    let c = cfg(0, 2);
    let mut rig = Rig::new(&c);
    // 0: two synths, a tempo-synced delay and a mixer.
    let s1 = rig.insert(0, 1, poly_synth::factory(), &[("osc1.wave", 1.0)]);
    let d2 = rig.insert(
        0,
        2,
        stereo_delay::factory(),
        &[("time.mode", 1.0), ("time.sync", 4.0), ("feedback", 0.5)],
    );
    let s3 = rig.insert(0, 3, poly_synth::factory(), &[("osc1.wave", 2.0)]);
    let m4 = rig.insert(0, 4, line_mixer::factory(), &[]);
    rig.stereo(s1, "out_", d2, "in_");
    rig.stereo(d2, "out_", m4, "ch1_in_");
    rig.stereo(s3, "out_", m4, "ch2_in_");
    rig.connect((m4, "master_l"), (Graph::INTERFACE, "out_1"));
    rig.connect((m4, "master_r"), (Graph::INTERFACE, "out_2"));
    rig.route(0);
    // 700: a timed param change.
    rig.param(700, d2, "feedback", 0.7);
    // 1500: insert a second delay into chain 2.
    let d5 = rig.insert(
        1500,
        5,
        stereo_delay::factory(),
        &[("time.mode", 0.0), ("time.ms", 23.0), ("feedback", 0.4)],
    );
    rig.disconnect((m4, "ch2_in_l"));
    rig.disconnect((m4, "ch2_in_r"));
    rig.stereo(s3, "out_", d5, "in_");
    rig.stereo(d5, "out_", m4, "ch2_in_");
    rig.route(1500);
    // 2300: tempo change (the synced delay follows).
    rig.tempo(2300, 150.0);
    // 3100: synth 3 leaves the routing, then the arena.
    rig.unroute_and_remove(3100, s3);
    // 3900: a mixer param, a new synth on channel 3.
    rig.param(3900, m4, "ch1.level", 0.5);
    let s6 = rig.insert(3900, 6, poly_synth::factory(), &[("osc1.wave", 3.0)]);
    rig.stereo(s6, "out_", m4, "ch3_in_");
    rig.route(3900);
    // 4700: delay 5 (still routed, fed by nothing since 3100) leaves the routing and the arena.
    rig.unroute_and_remove(4700, d5);
    // 5500: tempo again; delay 2 leaves the routing but keeps its state.
    rig.tempo(5500, 90.0);
    rig.disconnect((m4, "ch1_in_l"));
    rig.disconnect((m4, "ch1_in_r"));
    rig.disconnect((d2, "in_l"));
    rig.disconnect((d2, "in_r"));
    rig.topo.remove_node(d2).unwrap();
    rig.stereo(s1, "out_", m4, "ch1_in_");
    rig.route(5500);
    // 6300: delay 2 comes back (same instance, same tail).
    rig.topo
        .add_node(d2, stereo_delay::factory().descriptor)
        .unwrap();
    rig.disconnect((m4, "ch1_in_l"));
    rig.disconnect((m4, "ch1_in_r"));
    rig.stereo(s1, "out_", d2, "in_");
    rig.stereo(d2, "out_", m4, "ch1_in_");
    rig.route(6300);
    let notes = vec![
        note(0, s1, true, 60),
        note(10, s3, true, 55),
        note(1200, s3, false, 55),
        note(1600, s3, true, 57),
        note(2000, s1, true, 64),
        note(3200, s3, true, 59), // after its removal: dropped, deterministically
        note(4000, s6, true, 48),
        note(4500, s1, false, 64),
        note(7000, s6, false, 48),
    ];
    (rig, notes)
}

fn render(threads: u16, block: usize) -> Vec<Vec<f32>> {
    let (rig, notes) = script();
    let job = OfflineJob {
        frames: FRAMES,
        host_block: block,
        notes: &notes,
        params: &[],
        inputs: &[],
    };
    let baseline = worker_violations();
    let (mut engine, mut handle) = engine_with(cfg(0, 2), threads, Some(forbid_on_this_thread));
    let out = render_offline_scripted(&mut engine, &mut handle, &job, rig.edits, |f| {
        assert_no_alloc(f)
    });
    // Workers are still running here (they are joined when the engine drops).
    assert_eq!(
        worker_violations() - baseline,
        0,
        "threads {threads}, block {block}"
    );
    drop(engine);
    out
}

#[test]
fn ac_u3_scripted_hot_edits_are_bit_identical_and_allocation_free() {
    let reference = render(0, 64);
    assert!(
        energy(&reference[0], 0..FRAMES) > 1.0,
        "the script is audible"
    );
    // The edits are audible: the halves around the delay swap differ from silence.
    assert!(energy(&reference[0], 5500..6300) > 0.1);
    for threads in [0u16, 3] {
        for block in [1usize, 64, 333] {
            for run in 0..2 {
                let out = render(threads, block);
                assert_eq!(
                    bits(&out),
                    bits(&reference),
                    "threads {threads}, block {block}, run {run}"
                );
            }
        }
    }
}
