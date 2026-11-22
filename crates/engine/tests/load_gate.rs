//! The load rule: a live engine uses its render workers only while the audio thread needs help
//! to keep up; an offline one hands every level over. Costs come from a fake clock the devices
//! themselves advance, so no test depends on how fast the machine is: assertions are on
//! counters, on the engine's busy state and on what ran on the audio thread (the test thread
//! is the audio thread, so its fake clock reads exactly the work done at home), within bounds
//! counted in batches, never on durations.
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

fn counters(engine: &Engine) -> (u64, u64) {
    (engine.worker_idle_spins(), engine.worker_parks())
}

/// Waits until both worker counters stand still and every worker has parked at least once.
fn rest(engine: &Engine, threads: u64) -> (u64, u64) {
    wait_until("the workers came to rest", || {
        let before = counters(engine);
        std::thread::sleep(Duration::from_millis(100));
        before.1 >= threads && before == counters(engine)
    });
    counters(engine)
}

/// 32 devices a level, 64 in all.
const WIDE: usize = 32;

/// A batch that ran wholly on the audio thread advances its clock by the plan's whole cost; a
/// plan spread over two workers and the audio thread, by about a third of it. Anything over
/// this share of the whole is a batch that did not get the help the plan needs.
fn alone(home: u64, total: u64) -> bool {
    home * 100 >= total * 70
}

#[test]
fn many_cheap_nodes_under_the_leave_share_park_every_worker() {
    // 14 % of the budget together: far under the leave share, yet each level would save tens
    // of microseconds by being spread, which a rule that looked at levels one by one would act on.
    let cost = node_cost_for(2 * WIDE, 14);
    assert!(
        cost * WIDE as u64 > 50_000,
        "a level is worth spreading by itself"
    );
    let mut rig = rig(2, WIDE, cost, true, None, false);
    // A new plan is busy until its devices have a cost and the leave dwell has passed.
    batch(&mut rig.engine);
    assert!(rig.engine.workers_busy());
    until(&mut rig.engine, CALM_BOUND_BATCHES, "calm", |e| {
        !e.workers_busy()
    });
    let quiet = rest(&rig.engine, 2);
    for _ in 0..400 {
        batch(&mut rig.engine);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(!rig.engine.workers_busy());
    assert_eq!(
        counters(&rig.engine),
        quiet,
        "no worker woken, none spun, none parked again"
    );
}

#[test]
fn the_same_plan_offline_hands_every_level_over() {
    // The control of the test above, and the offline rule: no deadline, so the load rule is
    // never used and the workers wake on every callback.
    let cost = node_cost_for(2 * WIDE, 14);
    let mut rig = rig(2, WIDE, cost, false, None, false);
    for _ in 0..20 {
        batch(&mut rig.engine);
    }
    let quiet = rest(&rig.engine, 2);
    for _ in 0..CALM_BOUND_BATCHES {
        batch(&mut rig.engine);
    }
    wait_until("the woken workers parked again", || {
        rig.engine.worker_parks() > quiet.1
    });
}

#[test]
fn a_plan_that_turns_heavy_is_busy_within_a_few_batches_and_uses_the_workers() {
    let mut rig = rig(2, WIDE, node_cost_for(2 * WIDE, 5), true, None, false);
    until(&mut rig.engine, CALM_BOUND_BATCHES, "calm", |e| {
        !e.workers_busy()
    });
    let quiet = rest(&rig.engine, 2);
    // 90 % of the budget: the smoothed cost is over the enter share after one sample.
    rig.set_cost(node_cost_for(2 * WIDE, 90));
    let took = until(&mut rig.engine, ENTER_BOUND_BATCHES, "busy", |e| {
        e.workers_busy()
    });
    assert!(took <= ENTER_BOUND_BATCHES);
    for _ in 0..20 {
        batch(&mut rig.engine);
    }
    wait_until("the workers were woken and parked again", || {
        rig.engine.worker_parks() > quiet.1
    });
}

#[test]
fn costs_that_are_higher_on_a_worker_do_not_keep_a_calm_plan_busy() {
    // At home the plan takes 16 % of the budget; on a worker every node takes three times as
    // long (its data has moved to another core). Timed where it runs a busy plan would read
    // as far more, which is why it is also timed inline, one batch in 64: it must calm down.
    let mut rig = rig(2, WIDE, node_cost_for(2 * WIDE, 16), true, None, false);
    rig.on_worker.store(300, Ordering::Relaxed);
    batch(&mut rig.engine);
    assert!(rig.engine.workers_busy());
    until(&mut rig.engine, CALM_BOUND_BATCHES, "calm", |e| {
        !e.workers_busy()
    });
    // And it stays calm: every batch is timed at home now.
    for k in 0..500 {
        batch(&mut rig.engine);
        assert!(!rig.engine.workers_busy(), "batch {k}");
    }
}

#[test]
fn a_plan_that_reads_over_the_share_on_the_workers_stays_busy_whatever_it_costs_at_home() {
    // 20 % at home and five times as long on a worker: the batches timed where they ran read
    // about 73 % of the budget. Whether that is inflation or load cannot be told without an
    // inline batch, and an inline batch at a seen 73 % is not safe, so the plan stays busy and
    // neither probes nor counts calm time. Staying busy is the safe side.
    let mut rig = rig(2, WIDE, node_cost_for(2 * WIDE, 20), true, None, false);
    rig.on_worker.store(500, Ordering::Relaxed);
    let total = rig.total_cost();
    batch(&mut rig.engine);
    rig.home_batch();
    for k in 0..(2 * CALM_BOUND_BATCHES) {
        let home = rig.home_batch();
        assert!(rig.engine.workers_busy(), "batch {k}");
        assert!(!alone(home, total), "batch {k}: no inline probe");
    }
}

#[test]
fn a_plan_that_turns_light_stays_busy_for_most_of_a_second_and_then_calms_down() {
    let mut rig = rig(2, WIDE, node_cost_for(2 * WIDE, 60), true, None, false);
    for _ in 0..50 {
        batch(&mut rig.engine);
    }
    assert!(rig.engine.workers_busy());
    rig.set_cost(node_cost_for(2 * WIDE, 5));
    for k in 0..STILL_BUSY_BATCHES {
        batch(&mut rig.engine);
        assert!(rig.engine.workers_busy(), "batch {k}: leaving is slow");
    }
    until(&mut rig.engine, CALM_BOUND_BATCHES, "calm", |e| {
        !e.workers_busy()
    });
    // And back: the rise is noticed at once.
    rig.set_cost(node_cost_for(2 * WIDE, 90));
    until(&mut rig.engine, ENTER_BOUND_BATCHES, "busy again", |e| {
        e.workers_busy()
    });
}

#[test]
fn a_load_between_the_shares_changes_nothing() {
    let mut rig = rig(1, 4, node_cost_for(8, 5), true, None, false);
    until(&mut rig.engine, CALM_BOUND_BATCHES, "calm", |e| {
        !e.workers_busy()
    });
    // 25 % of the budget: over the leave share, under the enter share.
    rig.set_cost(node_cost_for(8, 25));
    for k in 0..(CALM_BOUND_BATCHES / 2) {
        batch(&mut rig.engine);
        assert!(!rig.engine.workers_busy(), "calm stays calm, batch {k}");
    }
    rig.set_cost(node_cost_for(8, 90));
    until(&mut rig.engine, ENTER_BOUND_BATCHES, "busy", |e| {
        e.workers_busy()
    });
    rig.set_cost(node_cost_for(8, 25));
    for k in 0..(2 * CALM_BOUND_BATCHES) {
        batch(&mut rig.engine);
        assert!(rig.engine.workers_busy(), "busy stays busy, batch {k}");
    }
}

#[test]
fn a_busy_plan_that_gets_heavier_never_runs_a_batch_on_the_audio_thread_alone() {
    // Busy at 40 %, then 150 %: a plan that needs the workers to keep up. Its inline probe
    // (one batch in 64) must not run at the old figure: from the second batch after the rise
    // the batches timed on the workers read over the share and no probe happens. (The first
    // batch after the rise may coincide with a probe, which is bounded by the 40 % just
    // measured; that one is not asserted.)
    let mut rig = rig(2, WIDE, node_cost_for(2 * WIDE, 40), true, None, false);
    for _ in 0..200 {
        batch(&mut rig.engine);
    }
    assert!(rig.engine.workers_busy());
    rig.set_cost(node_cost_for(rig.nodes, 150));
    let total = rig.total_cost();
    rig.home_batch();
    for k in 0..300 {
        let home = rig.home_batch();
        assert!(!alone(home, total), "batch {k}: {home} of {total} at home");
        assert!(rig.engine.workers_busy(), "batch {k}");
    }
}

#[test]
fn the_end_of_the_hold_after_play_never_runs_a_batch_alone_for_a_plan_that_got_heavy() {
    // Light while stopped (calm), 150 % of the budget once it plays: it needs the workers to
    // play at all. No batch of the hold, of its end or of the seconds after runs alone, and the
    // plan never goes calm.
    let mut rig = rig(2, WIDE, node_cost_for(2 * WIDE, 5), true, None, false);
    until(&mut rig.engine, CALM_BOUND_BATCHES, "calm", |e| {
        !e.workers_busy()
    });
    rig.set_cost(node_cost_for(rig.nodes, 150));
    rig.play();
    let total = rig.total_cost();
    for k in 0..1_300 {
        let home = rig.home_batch();
        assert!(!alone(home, total), "batch {k}: {home} of {total} at home");
        assert!(rig.engine.workers_busy(), "batch {k}");
    }
}

#[test]
fn a_plan_that_needed_help_while_playing_keeps_the_workers_for_as_long_as_it_plays() {
    let mut rig = rig(2, WIDE, node_cost_for(2 * WIDE, 90), true, None, false);
    rig.play();
    // Dense for a while: the plan has needed the workers while playing.
    for _ in 0..1_500 {
        batch(&mut rig.engine);
    }
    assert!(rig.engine.workers_busy());
    // A long quiet section of the song: still busy, so a dense onset is not taken inline.
    rig.set_cost(node_cost_for(rig.nodes, 2));
    for k in 0..(4 * CALM_BOUND_BATCHES) {
        batch(&mut rig.engine);
        assert!(rig.engine.workers_busy(), "quiet batch {k}");
    }
    // Stopped, it leaves as any plan does.
    rig.stop();
    until(&mut rig.engine, CALM_BOUND_BATCHES, "calm", |e| {
        !e.workers_busy()
    });
    // The next Play holds, and then stays busy over the same quiet plan.
    rig.play();
    for k in 0..(2 * CALM_BOUND_BATCHES) {
        batch(&mut rig.engine);
        assert!(rig.engine.workers_busy(), "second play, batch {k}");
    }
}

#[test]
fn a_plan_that_never_needed_help_while_playing_plays_inline() {
    // The control: the same quiet plan, playing from the start, goes calm after the hold and
    // the second under the leave share, and runs wholly at home.
    let mut rig = rig(2, WIDE, node_cost_for(2 * WIDE, 2), true, None, false);
    rig.play();
    until(&mut rig.engine, 3 * CALM_BOUND_BATCHES, "calm", |e| {
        !e.workers_busy()
    });
    let total = rig.total_cost();
    let home = rig.home_batch();
    assert!(home * 100 >= total * 95, "inline: {home} of {total}");
}

#[test]
fn levels_of_pinned_devices_never_wake_a_worker() {
    // A busy plan (90 % of the budget) whose devices all stay on the audio thread: there is
    // nothing a worker could take, so none is woken.
    let mut rig = rig(2, 8, node_cost_for(16, 90), true, None, true);
    for _ in 0..50 {
        batch(&mut rig.engine);
    }
    assert!(rig.engine.workers_busy());
    let quiet = rest(&rig.engine, 2);
    for _ in 0..400 {
        batch(&mut rig.engine);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(rig.engine.workers_busy());
    assert_eq!(
        counters(&rig.engine),
        quiet,
        "busy, but nothing to hand over"
    );
    // The control: the same plan with unpinned devices does use them.
    let mut free = common::costed::rig(2, 8, node_cost_for(16, 90), true, None, false);
    for _ in 0..50 {
        batch(&mut free.engine);
    }
    let quiet = rest(&free.engine, 2);
    for _ in 0..20 {
        batch(&mut free.engine);
    }
    wait_until("the workers took part", || {
        free.engine.worker_parks() > quiet.1
    });
}

/// Renders `batches` batches, switching the cost of every device between heavy and light every
/// `period` batches (0: never), and returns the bit patterns of the output.
fn render_with(rig: &mut Rig, batches: usize, period: usize) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); 4];
    for k in 0..batches {
        if period > 0 && k.is_multiple_of(period) {
            let share = if (k / period).is_multiple_of(2) {
                90
            } else {
                5
            };
            rig.set_cost(node_cost_for(rig.nodes, share));
        }
        for (o, ch) in out.iter_mut().zip(batch(&mut rig.engine)) {
            o.extend(bits(&[ch])[0].iter());
        }
    }
    out
}

#[test]
fn the_output_is_identical_whichever_rule_decides() {
    let n = 2_500;
    let width = 16;
    let light = node_cost_for(2 * width, 5);
    let reference = render_with(&mut rig(0, width, light, true, None, false), n, 0);
    assert!(reference.iter().flatten().any(|&b| b != 0));
    for (label, realtime, mode, period, play) in [
        ("inline", true, Some(GateMode::ForceInline), 0, false),
        ("dispatch", true, Some(GateMode::ForceDispatch), 0, false),
        ("live rule, light", true, None, 0, false),
        ("live rule, heavy then light", true, None, 1_100, false),
        (
            "live rule, playing, heavy then light",
            true,
            None,
            1_100,
            true,
        ),
        ("offline rule", false, None, 0, false),
        ("offline rule, costs changing", false, None, 1_100, false),
        (
            "load rule forced offline",
            false,
            Some(GateMode::Load),
            1_100,
            false,
        ),
    ] {
        let mut r = rig(2, width, light, realtime, mode, false);
        if play {
            r.play();
        }
        let mut flips = 0;
        let mut last = r.engine.workers_busy();
        let mut got: Vec<Vec<u32>> = vec![Vec::new(); 4];
        for chunk in 0..(n / 50) {
            if period > 0 && (chunk * 50).is_multiple_of(period) {
                let share = if (chunk * 50 / period).is_multiple_of(2) {
                    90
                } else {
                    5
                };
                r.set_cost(node_cost_for(r.nodes, share));
            }
            for _ in 0..50 {
                for (g, ch) in got.iter_mut().zip(batch(&mut r.engine)) {
                    g.extend(bits(&[ch])[0].iter());
                }
            }
            let now = r.engine.workers_busy();
            flips += usize::from(now != last);
            last = now;
        }
        assert_eq!(got, reference, "{label}");
        if label == "live rule, heavy then light" {
            assert!(
                flips >= 2,
                "the busy state did flip during the run: {flips}"
            );
        }
    }
}
