//! The level gate allocates, locks and logs nothing: a run in which every level flips between
//! inline and dispatched several times, with the audio thread and every worker guarded. Own
//! binary: the worker violation count is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_testkit::no_alloc::worker_violations;
use common::costed::*;
use std::sync::atomic::Ordering;

#[test]
fn flipping_the_mode_of_a_level_allocates_on_no_thread() {
    let Rig {
        mut engine, costs, ..
    } = rig(2, None);
    let mut flips = 0;
    let mut last = Vec::new();
    for phase in 0..6 {
        let cost = if phase % 2 == 0 { HEAVY } else { CHEAP };
        costs.iter().for_each(|c| c.store(cost, Ordering::Relaxed));
        // Every batch is guarded on the audio thread by `render_block`; the workers are guarded
        // from their start (`forbid_on_this_thread`).
        for _ in 0..FLIP_BOUND_BATCHES {
            batch(&mut engine);
            let now = engine.level_modes();
            flips += usize::from(!last.is_empty() && now != last);
            last = now;
        }
    }
    assert!(flips >= 4, "the levels flipped during the run: {flips}");
    assert_eq!(worker_violations(), 0);
}
