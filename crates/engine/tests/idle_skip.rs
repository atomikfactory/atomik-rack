//! The idle skip: a device that says it is quiescent is not called while its inputs stay
//! silent, and a render with the skip on is byte-for-byte the render with it off. Tests cover
//! each condition of the rule on its own, a free-running device across silence (and a device
//! whose `idle_advance` is wrong, which must be caught), worker counts, and the load rule:
//! idle devices wake no worker, and a batch in which they wake hands the same nodes to the
//! workers as it would without the skip. No test depends on how fast the machine is: costs are
//! fake, assertions are on counters and bytes.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::Ticks;
use atomik_engine::TransportCommand;
use common::idle::*;
use std::time::Duration;

fn settled(layout: Layout, song: bool) -> World {
    let mut o = Options::new(layout, 1);
    o.song = song;
    let mut w = world(&o);
    w.run(4);
    assert_eq!(w.processes(0), 1, "called once, then found quiescent");
    assert_eq!(w.skipped(0), 3);
    assert_eq!(w.advances(0), 3, "idle_advance once per skipped batch");
    w
}

#[test]
fn a_quiescent_device_with_silent_inputs_is_skipped_from_its_second_batch() {
    let mut w = settled(Layout::Sources, false);
    w.run(100);
    assert_eq!(w.processes(0), 1);
    assert_eq!(w.skipped(0), 103);
    assert_eq!(w.advances(0), 103);
    assert_eq!(
        w.probes[0]
            .counts
            .advanced_frames
            .load(std::sync::atomic::Ordering::Relaxed),
        103 * 64,
        "a whole batch of frames each time"
    );
}

/// Breaking only one condition makes the node run exactly once, and the node is skipped again
/// as soon as the condition is back (a device asked again after a run).
fn wakes_once(name: &str, song: bool, event: impl FnOnce(&mut World), then: impl FnOnce(&World)) {
    let mut w = settled(Layout::Sources, song);
    let (p, s) = (w.processes(0), w.skipped(0));
    event(&mut w);
    w.batch();
    assert_eq!(w.processes(0), p + 1, "{name}: the device ran");
    assert_eq!(w.skipped(0), s, "{name}: and was not skipped");
    then(&w);
    w.run(2);
    assert_eq!(w.processes(0), p + 1, "{name}: quiet again, skipped again");
    assert!(w.skipped(0) > s, "{name}");
}

#[test]
fn sound_on_the_audio_input_makes_the_node_run() {
    wakes_once(
        "noise",
        false,
        |w| w.set_source(0, NOISE),
        |w| w.set_source(0, SILENCE),
    );
}

#[test]
fn a_negative_zero_is_not_silence() {
    wakes_once(
        "-0.0",
        false,
        |w| w.set_source(0, NEGATIVE_ZERO),
        |w| w.set_source(0, SILENCE),
    );
}

#[test]
fn a_cv_value_on_the_cv_input_makes_the_node_run() {
    wakes_once(
        "cv",
        false,
        |w| w.set_source(0, CV_ONLY),
        |w| w.set_source(0, SILENCE),
    );
}

#[test]
fn a_note_makes_the_node_run() {
    wakes_once("note", false, |w| w.note(0), |_| ());
}

#[test]
fn a_parameter_change_makes_the_node_run() {
    wakes_once("param", false, |w| w.param(0, 0.5), |_| ());
}

#[test]
fn a_tempo_change_makes_the_node_run() {
    wakes_once("tempo", false, |w| w.tempo(133.0), |_| ());
}

#[test]
fn starting_and_stopping_the_transport_makes_the_node_run() {
    wakes_once(
        "play",
        true,
        |w| w.transport(TransportCommand::Play),
        |_| (),
    );
    // A playing transport only advances: that is steady too. Stop is a change of its own.
    let mut w = settled(Layout::Sources, true);
    w.transport(TransportCommand::Play);
    w.run(3);
    let (p, s) = (w.processes(0), w.skipped(0));
    w.run(20);
    assert_eq!(w.processes(0), p, "a playing transport is only advancing");
    assert_eq!(w.skipped(0), s + 20);
    w.transport(TransportCommand::Stop);
    w.batch();
    assert_eq!(w.processes(0), p + 1, "stop");
}

#[test]
fn a_jump_of_the_position_makes_the_node_run() {
    wakes_once(
        "locate while stopped",
        true,
        |w| w.transport(TransportCommand::Locate(Ticks(1_920))),
        |_| (),
    );
    let mut w = settled(Layout::Sources, true);
    w.transport(TransportCommand::Play);
    w.run(6);
    let p = w.processes(0);
    w.transport(TransportCommand::Locate(Ticks(3_840)));
    w.batch();
    assert_eq!(w.processes(0), p + 1, "locate while playing");
}

#[test]
fn a_loop_wrap_makes_the_node_run() {
    let mut w = settled(Layout::Sources, true);
    // Half a beat is 0.25 s at 120 BPM: 187.5 batches.
    w.transport(TransportCommand::SetLoop {
        start: Ticks(0),
        end: Ticks(atomik_core::PPQ / 2),
        enabled: true,
    });
    w.transport(TransportCommand::Play);
    w.run(6);
    let before = w.processes(0);
    w.run(400);
    let runs = w.processes(0) - before;
    assert!(
        (2..=6).contains(&runs),
        "two runs for each wrap (the wrap and the batch after it), not every batch: {runs}"
    );
}

#[test]
fn a_node_that_is_previewing_on_its_own_clock_runs_every_batch() {
    let mut w = settled(Layout::Sources, true);
    let id = w.probes[0].id;
    w.handle.preview_player(id, true).unwrap();
    let p = w.processes(0);
    w.run(10);
    assert_eq!(w.processes(0), p + 10);
    w.handle.preview_player(id, false).unwrap();
    w.run(6);
    assert_eq!(w.processes(0), p + 10 + 1, "one run, then skipped again");
}

#[test]
fn a_device_that_says_it_is_not_quiescent_is_called_every_batch() {
    let mut w = settled(Layout::Sources, false);
    w.probes[0]
        .set
        .answer
        .store(false, std::sync::atomic::Ordering::Relaxed);
    // The answer is read after the next run: a param makes it run.
    w.param(0, 1.0);
    let p = w.processes(0);
    w.run(20);
    assert_eq!(w.processes(0), p + 20);
    assert_eq!(w.advances(0), 3, "idle_advance is not called");
}

const STAGED: StagedOptions = StagedOptions {
    skip: true,
    publishes: false,
    ignore_ret: false,
};

/// The bit patterns of what a staged world renders over `batches` batches, the sound of the
/// two sources given by `modes(batch)`.
fn staged_bits(o: StagedOptions, batches: usize, modes: impl Fn(usize) -> (u32, u32)) -> Vec<u32> {
    let (mut w, _, _) = staged_world(o);
    let mut bits = Vec::new();
    for n in 0..batches {
        let (a, b) = modes(n);
        w.set_source(0, a);
        w.set_source(1, b);
        push_bits(&mut bits, &w.batch());
    }
    bits
}

#[test]
fn the_stages_of_a_quiescent_device_are_skipped_one_by_one() {
    let (mut w, counts, id) = staged_world(STAGED);
    w.run(10);
    assert_eq!(
        (counts.process(0), counts.process(1)),
        (1, 1),
        "both stages once, then found quiescent"
    );
    assert_eq!((counts.advance(0), counts.advance(1)), (9, 9));
    assert_eq!(w.engine.idle_skipped(id), Some(9), "a batch counts once");
}

#[test]
fn sound_on_the_first_stage_input_runs_the_stage_after_it_too() {
    let (mut w, counts, _) = staged_world(STAGED);
    w.run(4);
    w.set_source(0, NOISE);
    w.batch();
    assert_eq!(
        (counts.process(0), counts.process(1)),
        (2, 2),
        "stage 0 ran, so stage 1 ran with its input silent"
    );
    w.set_source(0, SILENCE);
    w.run(3);
    assert_eq!(
        (counts.process(0), counts.process(1)),
        (2, 2),
        "quiet again"
    );
    assert_eq!((counts.advance(0), counts.advance(1)), (3 + 3, 3 + 3));
}

#[test]
fn sound_on_the_second_stage_input_runs_that_stage_alone() {
    let (mut w, counts, id) = staged_world(STAGED);
    w.run(4);
    let skipped = w.engine.idle_skipped(id).unwrap();
    w.set_source(1, NOISE);
    w.batch();
    assert_eq!(
        (counts.process(0), counts.process(1)),
        (1, 2),
        "only stage 1 ran"
    );
    assert_eq!(counts.advance(0), 3 + 1, "stage 0 was advanced");
    assert_eq!(
        w.engine.idle_skipped(id),
        Some(skipped),
        "a batch whose last stage ran is not a skipped one"
    );
    w.set_source(1, SILENCE);
    w.run(2);
    assert_eq!((counts.process(0), counts.process(1)), (1, 2));
}

#[test]
fn a_stage_that_publishes_its_state_runs_without_waking_the_others() {
    let (mut w, counts, _) = staged_world(StagedOptions {
        publishes: true,
        ..STAGED
    });
    w.run(10);
    assert_eq!(counts.process(0), 10, "stage 0 every batch");
    assert_eq!(counts.process(1), 1, "stage 1 once");
    assert_eq!(counts.advance(0), 0);
    assert_eq!(counts.advance(1), 9);
}

#[test]
fn an_input_the_device_ignores_does_not_keep_its_stage_running() {
    let (mut w, counts, _) = staged_world(StagedOptions {
        ignore_ret: true,
        ..STAGED
    });
    w.run(2);
    w.set_source(1, NOISE);
    w.run(5);
    assert_eq!(
        (counts.process(0), counts.process(1)),
        (1, 1),
        "the return is not looked at"
    );
    let (mut w, counts, _) = staged_world(STAGED);
    w.run(2);
    w.set_source(1, NOISE);
    w.run(5);
    assert_eq!(counts.process(1), 1 + 5, "looked at, the stage runs");
}

#[test]
fn a_staged_render_is_the_same_skipped_or_not() {
    let modes = |n: usize| match n {
        10..=14 => (NOISE, SILENCE),
        20..=24 => (SILENCE, NOISE),
        30..=32 => (NOISE, NOISE),
        40 => (NEGATIVE_ZERO, SILENCE),
        _ => (SILENCE, SILENCE),
    };
    let on = staged_bits(STAGED, 50, modes);
    let off = staged_bits(
        StagedOptions {
            skip: false,
            ..STAGED
        },
        50,
        modes,
    );
    assert!(on.iter().any(|&b| b != 0));
    assert_eq!(on, off);
}

#[test]
fn the_switch_turns_the_skip_off_and_the_device_is_called_every_batch() {
    let mut o = Options::new(Layout::Sources, 1);
    o.skip = false;
    let mut w = world(&o);
    w.run(30);
    assert_eq!(w.processes(0), 30);
    assert_eq!(w.advances(0), 0);
    assert_eq!(w.skipped(0), 0);
    // And back on, in the middle of a run.
    w.engine.set_idle_skip(true);
    w.run(10);
    assert!(w.skipped(0) >= 8, "{}", w.skipped(0));
}

#[test]
fn meter_values_and_latency_questions_do_not_wake_the_device() {
    let mut o = Options::new(Layout::Sources, 1);
    o.meters = true;
    let mut w = world(&o);
    w.run(50);
    assert_eq!(w.processes(0), 1, "asked every batch, still skipped");
    assert_eq!(w.skipped(0), 49);
}

#[test]
fn a_device_that_panics_in_idle_advance_is_a_contained_fault() {
    let mut w = settled(Layout::Sources, false);
    let id = w.probes[0].id;
    w.probes[0]
        .set
        .panic_idle
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let skipped = w.skipped(0);
    for _ in 0..5 {
        w.raw_batch();
    }
    assert_eq!(w.engine.fault(), Some(id));
    assert_eq!(w.advances(0), 4, "the failing call, and no call after it");
    assert_eq!(w.skipped(0), skipped, "a failed batch is not a skipped one");
    assert_eq!(w.processes(0), 1);
}

#[test]
fn a_faulted_node_is_not_skipped_and_not_called() {
    let mut w = settled(Layout::Sources, false);
    let id = w.probes[0].id;
    w.probes[0]
        .set
        .panic_sound
        .store(true, std::sync::atomic::Ordering::Relaxed);
    w.set_source(0, NOISE);
    w.raw_batch();
    assert_eq!(w.engine.fault(), Some(id));
    w.set_source(0, SILENCE);
    let (p, a, s) = (w.processes(0), w.advances(0), w.skipped(0));
    for _ in 0..10 {
        w.raw_batch();
    }
    assert_eq!(
        (w.processes(0), w.advances(0), w.skipped(0)),
        (p, a, s),
        "silent from then on, neither processed nor advanced nor counted as skipped"
    );
}

/// What a scenario rendered and counted.
#[derive(PartialEq, Debug)]
struct Outcome {
    bits: Vec<u32>,
    skipped: Vec<u64>,
    processes: Vec<u64>,
}

/// Sound, silence long enough to be skipped, sound again, with a note, a parameter and two
/// tempo changes in the silence. `right` says whether `idle_advance` of the probes does what
/// `process` does to the phase.
fn scenario(threads: u16, rule: Rule, skip: bool, right: bool, meters: bool) -> Outcome {
    const WIDTH: usize = 6;
    let mut o = Options::new(Layout::Sources, WIDTH);
    o.threads = threads;
    o.rule = rule;
    o.skip = skip;
    o.right = right;
    o.meters = meters;
    let mut w = world(&o);
    let mut bits = Vec::new();
    for b in 0..400u64 {
        for k in 0..WIDTH {
            let sound = b < 20
                || (k % 2 == 0 && (150..170).contains(&b))
                || (k % 3 == 0 && (300..310).contains(&b));
            w.set_source(k, if sound { NOISE } else { SILENCE });
        }
        match b {
            60 => w.note(1),
            90 => w.param(2, 0.5),
            120 => w.tempo(133.0),
            250 => w.tempo(120.0),
            330 => w.param(0, 1.5),
            _ => {}
        }
        push_bits(&mut bits, &w.batch());
    }
    Outcome {
        bits,
        skipped: (0..WIDTH).map(|k| w.skipped(k)).collect(),
        processes: (0..WIDTH).map(|k| w.processes(k)).collect(),
    }
}

#[test]
fn a_free_running_device_renders_the_same_bytes_with_the_skip_on_and_off() {
    for meters in [false, true] {
        let off = scenario(0, Rule::Offline, false, true, meters);
        let on = scenario(0, Rule::Offline, true, true, meters);
        assert!(off.skipped.iter().all(|&s| s == 0));
        assert!(
            on.skipped.iter().all(|&s| s >= 100),
            "the scenario really skips: {:?}",
            on.skipped
        );
        assert!(on.bits.iter().any(|&b| b != 0), "and really sounds");
        assert_eq!(on.bits, off.bits, "meters {meters}");
    }
}

#[test]
fn a_wrong_idle_advance_is_caught_by_the_same_comparison() {
    let reference = scenario(0, Rule::Offline, false, true, false);
    let wrong = scenario(0, Rule::Offline, true, false, false);
    assert!(wrong.skipped.iter().all(|&s| s >= 100));
    assert_ne!(
        wrong.bits, reference.bits,
        "a device that does not advance its phase is heard"
    );
}

#[test]
fn any_worker_count_skips_the_same_batches_and_renders_the_same_bytes() {
    let reference = scenario(0, Rule::Offline, true, true, false);
    let off = scenario(0, Rule::Offline, false, true, false);
    assert_eq!(reference.bits, off.bits);
    for (threads, rule, name) in [
        (1, Rule::Offline, "1 worker, offline"),
        (4, Rule::Offline, "4 workers, offline"),
        (4, Rule::Live, "4 workers, live rule"),
        (0, Rule::Live, "no workers, live rule"),
    ] {
        let got = scenario(threads, rule, true, true, false);
        assert_eq!(got.skipped, reference.skipped, "{name}: skipped batches");
        assert_eq!(got.processes, reference.processes, "{name}: calls");
        assert_eq!(got.bits, reference.bits, "{name}: bytes");
    }
}

/// 16 probes at level 0 (instruments that take notes) and 16 at level 1, each feeding the
/// next; together they take 90 % of the batch budget on the fake clock, so the load rule is
/// busy and has two render workers.
fn heavy(skip: bool, song: bool) -> World {
    let nodes = 32;
    let mut o = Options::new(Layout::Chain, 16);
    o.threads = 2;
    o.rule = Rule::Live;
    o.cost = budget_ns() * 90 / 100 / nodes;
    o.skip = skip;
    o.song = song;
    world(&o)
}

/// Nanoseconds of audio in one batch at 48 kHz.
fn budget_ns() -> u64 {
    (atomik_core::BATCH as f64 * 1e9 / 48_000.0) as u64
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..4000 {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out: {what}");
}

fn counters(w: &World) -> (u64, u64) {
    (w.engine.worker_idle_spins(), w.engine.worker_parks())
}

/// Waits until both worker counters stand still and every worker has parked at least once.
fn rest(w: &World, threads: u64) -> (u64, u64) {
    wait_until("the workers came to rest", || {
        let before = counters(w);
        std::thread::sleep(Duration::from_millis(100));
        before.1 >= threads && before == counters(w)
    });
    counters(w)
}

#[test]
fn a_plan_of_idle_devices_wakes_no_worker_and_its_cost_does_not_decay() {
    let mut w = heavy(true, false);
    w.run(40);
    assert!(w.engine.workers_busy(), "90 % of the budget: busy");
    assert!((0..32).all(|i| w.skipped(i) > 0));
    let quiet = rest(&w, 2);
    // Two seconds of audio. A skipped batch is not timed and feeds nothing to the smoothed
    // cost: were it counted as free, the gate would calm down after one second.
    w.run(1_500);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        w.engine.workers_busy(),
        "the cost of the last real run stands"
    );
    assert_eq!(
        counters(&w),
        quiet,
        "no worker woken, none spun, none parked again"
    );
}

#[test]
fn the_same_plan_without_the_skip_keeps_waking_the_workers() {
    // The control of the test above: the counters do move when the devices run.
    let mut w = heavy(false, false);
    w.run(40);
    let quiet = rest(&w, 2);
    w.run(100);
    wait_until("the workers were woken and parked again", || {
        w.engine.worker_parks() > quiet.1
    });
}

/// Per node, how many calls of the batch after a quiet stretch ran on a render worker, once
/// `wake` has made the nodes wake up.
fn workers_in_the_waking_batch(skip: bool, song: bool, wake: impl FnOnce(&mut World)) -> Vec<u64> {
    let mut w = heavy(skip, song);
    w.run(60);
    rest(&w, 2);
    let before: Vec<u64> = (0..32).map(|i| w.on_worker(i)).collect();
    wake(&mut w);
    w.batch();
    (0..32).map(|i| w.on_worker(i) - before[i]).collect()
}

#[test]
fn the_batch_in_which_idle_devices_wake_gets_the_workers_it_would_get_without_the_skip() {
    let notes = |w: &mut World| (0..16).for_each(|i| w.note(i));
    let with = workers_in_the_waking_batch(true, false, notes);
    let without = workers_in_the_waking_batch(false, false, notes);
    assert!(
        with.iter().sum::<u64>() > 0,
        "the workers took part: {with:?}"
    );
    assert_eq!(with, without, "the same nodes went to the workers");
    // Level 1 woke by the sound level 0 made, and was handed over like level 0.
    assert!(with[16..].iter().sum::<u64>() > 0);
}

#[test]
fn pressing_play_over_idle_devices_gets_the_workers_it_would_get_without_the_skip() {
    let play = |w: &mut World| w.transport(TransportCommand::Play);
    let with = workers_in_the_waking_batch(true, true, play);
    let without = workers_in_the_waking_batch(false, true, play);
    assert!(with.iter().sum::<u64>() > 0, "{with:?}");
    assert_eq!(with, without);
}

#[test]
fn play_with_notes_for_every_instrument_in_the_same_batch_gets_the_same_workers() {
    let both = |w: &mut World| {
        w.transport(TransportCommand::Play);
        (0..16).for_each(|i| w.note(i));
    };
    let with = workers_in_the_waking_batch(true, true, both);
    let without = workers_in_the_waking_batch(false, true, both);
    assert!(with.iter().sum::<u64>() > 0);
    assert_eq!(with, without);
}

#[test]
fn only_the_nodes_that_wake_are_handed_over_when_some_stay_idle() {
    // Four of sixteen instruments get a note: the other twelve stay home and idle, and the
    // workers get exactly the nodes that run: no node that skips is ever counted as work.
    let mut w = heavy(true, false);
    w.run(60);
    let before: Vec<u64> = (0..32).map(|i| w.on_worker(i)).collect();
    let skipped: Vec<u64> = (0..32).map(|i| w.skipped(i)).collect();
    (0..4).for_each(|i| w.note(i));
    w.batch();
    for i in 0..32 {
        let woke = i < 4 || (16..20).contains(&i);
        assert_eq!(w.skipped(i) == skipped[i], woke, "node {i}");
        if !woke {
            assert_eq!(w.on_worker(i), before[i], "node {i}: idle nodes stay home");
        }
    }
}

#[test]
fn a_change_of_the_cabling_makes_a_skipped_node_run_once() {
    use atomik_core::BATCH;
    use atomik_device_sdk::SocketId;
    use atomik_engine::{DeviceInstanceId, Graph, Topology, new_engine};
    use common::{cfg, ep, render_block};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

    let c = cfg(0, 1);
    let (mut engine, mut handle) = new_engine(c);
    let mut t = Topology::new(&c);
    let (src, probe) = (DeviceInstanceId(1), DeviceInstanceId(2));
    let mode = Arc::new(AtomicU32::new(SILENCE));
    let counts = Arc::new(Counts::default());
    handle.insert_device(src, new_source(&mode), &[]).unwrap();
    handle
        .insert_device(probe, new_probe(&counts, &settings()), &[])
        .unwrap();
    t.add_node(src, &SRC).unwrap();
    t.add_node(probe, &PROBE).unwrap();
    t.connect(ep(src, SocketId(0)), ep(probe, AUDIO_IN))
        .unwrap();
    t.connect(ep(src, SocketId(1)), ep(probe, CV_IN)).unwrap();
    t.connect(ep(probe, AUDIO_OUT), ep(Graph::INTERFACE, SocketId(0)))
        .unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    for _ in 0..10 {
        render_block(&mut engine, &[], 1, BATCH);
    }
    assert_eq!(counts.process.load(Relaxed), 1);
    assert_eq!(engine.idle_skipped(probe), Some(9));
    // The CV cable goes: the probe sees a different `is_connected`, so it runs once.
    t.disconnect(ep(probe, CV_IN)).unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    for _ in 0..10 {
        render_block(&mut engine, &[], 1, BATCH);
    }
    assert_eq!(
        counts.process.load(Relaxed),
        2,
        "one run for the new cabling"
    );
    assert_eq!(engine.idle_skipped(probe), Some(9 + 9));
}
