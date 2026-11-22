//! The level gate: the engine hands a level to the render workers only when the level is worth
//! it. Costs come from a fake clock the devices themselves advance, so no test depends on how
//! fast the machine is: assertions are on counters and on the mode of each level, within a
//! bound counted in batches, never on durations.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{Engine, GateMode};
use common::costed::*;
use common::*;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn wait_until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..4000 {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out: {what}");
}

/// Waits until both worker counters stand still and every worker has parked at least once.
fn rest(engine: &Engine, threads: u64) -> (u64, u64) {
    wait_until("the workers came to rest", || {
        let before = (engine.worker_idle_spins(), engine.worker_parks());
        std::thread::sleep(Duration::from_millis(100));
        before.1 >= threads && before == (engine.worker_idle_spins(), engine.worker_parks())
    });
    (engine.worker_idle_spins(), engine.worker_parks())
}

#[test]
fn a_plan_of_cheap_levels_never_wakes_a_worker() {
    let Rig { mut engine, .. } = rig(2, None);
    // A fresh level is dispatched until its devices have been timed: one batch.
    batch(&mut engine);
    assert_eq!(engine.level_modes(), vec![true, true]);
    until(&mut engine, "both levels inline", |e| {
        e.level_modes() == [false, false]
    });
    let quiet = rest(&engine, 2);
    for _ in 0..400 {
        batch(&mut engine);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(engine.level_modes(), vec![false, false]);
    assert_eq!(
        (engine.worker_idle_spins(), engine.worker_parks()),
        quiet,
        "no worker was woken, none spun, none parked again"
    );
}

#[test]
fn the_same_plan_forced_to_dispatch_does_wake_the_workers() {
    // The control of the test above: without the gate the workers wake on every callback.
    let Rig { mut engine, .. } = rig(2, Some(GateMode::ForceDispatch));
    for _ in 0..20 {
        batch(&mut engine);
    }
    let quiet = rest(&engine, 2);
    for _ in 0..20 {
        batch(&mut engine);
    }
    wait_until("the woken workers parked again", || {
        engine.worker_parks() > quiet.1
    });
}

#[test]
fn only_the_heavy_level_is_dispatched() {
    let Rig {
        mut engine, costs, ..
    } = rig(2, None);
    costs[1].store(HEAVY, Ordering::Relaxed);
    until(&mut engine, "level 1 dispatched, level 0 inline", |e| {
        e.level_modes() == [false, true]
    });
    for _ in 0..FLIP_BOUND_BATCHES {
        batch(&mut engine);
        assert_eq!(engine.level_modes(), vec![false, true], "no flapping");
    }
    // And the other way round.
    costs[0].store(HEAVY, Ordering::Relaxed);
    costs[1].store(CHEAP, Ordering::Relaxed);
    until(&mut engine, "level 0 dispatched, level 1 inline", |e| {
        e.level_modes() == [true, false]
    });
}

#[test]
fn a_level_that_becomes_heavy_is_dispatched_and_the_workers_park_again_when_it_is_cheap() {
    let Rig {
        mut engine, costs, ..
    } = rig(2, None);
    until(&mut engine, "inline", |e| e.level_modes() == [false, false]);
    let quiet = rest(&engine, 2);
    costs[0].store(HEAVY, Ordering::Relaxed);
    until(&mut engine, "level 0 dispatched", |e| e.level_modes()[0]);
    // The workers are woken for the dispatched level: they spin inside the call and park at its
    // end, which the park counter shows.
    for _ in 0..20 {
        batch(&mut engine);
    }
    wait_until("the workers were woken and parked again", || {
        engine.worker_parks() > quiet.1
    });
    costs[0].store(CHEAP, Ordering::Relaxed);
    until(&mut engine, "level 0 back inline", |e| {
        e.level_modes() == [false, false]
    });
    let quiet = rest(&engine, 2);
    for _ in 0..200 {
        batch(&mut engine);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        (engine.worker_idle_spins(), engine.worker_parks()),
        quiet,
        "cheap again: the workers stay parked"
    );
}

#[test]
fn a_hovering_cost_does_not_flap_the_mode() {
    // Four devices of 3 us, two threads: 6 us to save, between the leave and the enter thresholds, so a level
    // keeps the mode it has, whichever it is.
    for start in [CHEAP, HEAVY] {
        let Rig {
            mut engine, costs, ..
        } = rig(1, None);
        costs[0].store(start, Ordering::Relaxed);
        until(&mut engine, "level 0 settled", |e| {
            e.level_modes()[0] == (start == HEAVY)
        });
        let settled = engine.level_modes()[0];
        costs[0].store(3_000, Ordering::Relaxed);
        for _ in 0..(FLIP_BOUND_BATCHES * 2) {
            batch(&mut engine);
            assert_eq!(engine.level_modes()[0], settled, "start {start}");
        }
    }
}

/// Renders `batches` batches, switching the cost of both levels between heavy and cheap every
/// `period` batches (0: never), and returns the bit patterns of the output. `flips` counts the
/// changes of any level's mode seen between batches.
fn render_with(rig: &mut Rig, batches: usize, period: usize, flips: &mut usize) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); WIDTH];
    let mut last = rig.engine.level_modes();
    for k in 0..batches {
        if period > 0 && k.is_multiple_of(period) {
            let v = if (k / period).is_multiple_of(2) {
                HEAVY
            } else {
                CHEAP
            };
            rig.costs.iter().for_each(|c| c.store(v, Ordering::Relaxed));
        }
        for (o, ch) in out.iter_mut().zip(batch(&mut rig.engine)) {
            o.extend(bits(&[ch])[0].iter());
        }
        let now = rig.engine.level_modes();
        *flips += usize::from(now != last);
        last = now;
    }
    out
}

#[test]
fn the_output_is_identical_whichever_way_the_gate_decides() {
    let n = 1_500;
    let mut ignored = 0;
    let reference = render_with(&mut rig(0, None), n, 0, &mut ignored);
    assert!(reference.iter().flatten().any(|&b| b != 0));
    for (mode, period) in [
        (GateMode::ForceInline, 0),
        (GateMode::ForceDispatch, 0),
        (GateMode::Auto, 0),
        // The modes flip during the run: heavy for 300 batches, cheap for 300, and so on.
        (GateMode::Auto, 300),
    ] {
        let mut flips = 0;
        let got = render_with(&mut rig(2, Some(mode)), n, period, &mut flips);
        assert_eq!(got, reference, "{mode:?} period {period}");
        if period > 0 {
            assert!(flips >= 2, "the modes did flip during the run: {flips}");
        }
    }
}
