//! The idle skip allocates, locks and logs nothing: batches that skip, the batch in which
//! the skipped nodes wake and hand work to the workers, and the quiet after it, with the audio
//! thread and every worker guarded. Own binary: the worker violation count is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_testkit::no_alloc::worker_violations;
use common::idle::*;

#[test]
fn skipping_and_waking_allocate_on_no_thread() {
    let mut o = Options::new(Layout::Chain, 8);
    o.threads = 2;
    o.rule = Rule::Live;
    o.cost = 20_000;
    let mut w = world(&o);
    for round in 0..3 {
        // Every batch is guarded on the audio thread by `World::batch`; the workers are
        // guarded from their start.
        w.run(60);
        assert!((0..16).all(|i| w.skipped(i) > 0), "round {round}: skipping");
        for i in 0..8 {
            w.note(i);
        }
        w.param(3, 0.5);
        w.tempo(120.0 + f64::from(round));
        w.run(10);
    }
    assert_eq!(worker_violations(), 0);
}
