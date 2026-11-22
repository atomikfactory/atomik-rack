//! The load rule allocates, locks and logs nothing: a run in which the plan flips between calm
//! and busy several times, with the audio thread and every worker guarded. Own binary: the
//! worker violation count is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_testkit::no_alloc::worker_violations;
use common::costed::*;

#[test]
fn flipping_between_calm_and_busy_allocates_on_no_thread() {
    let width = 16;
    let mut rig = rig(2, width, node_cost_for(2 * width, 5), true, None, false);
    let mut flips = 0;
    let mut last = true;
    for phase in 0..4u32 {
        let share = if phase.is_multiple_of(2) { 90 } else { 5 };
        rig.set_cost(node_cost_for(rig.nodes, share));
        // Every batch is guarded on the audio thread by `render_block`; the workers are guarded
        // from their start (`forbid_on_this_thread`).
        for _ in 0..CALM_BOUND_BATCHES {
            batch(&mut rig.engine);
            let now = rig.engine.workers_busy();
            flips += usize::from(now != last);
            last = now;
        }
    }
    assert!(flips >= 2, "the plan flipped during the run: {flips}");
    assert_eq!(worker_violations(), 0);
}
