//! (on every thread): multicore rendering allocates neither on the audio thread
//! (`assert_no_alloc` around every render) nor on any worker (`forbid_on_this_thread` installed
//! through `on_thread_start`). Own binary: the worker violation counter is process-global, and
//! nothing in this binary may allocate on a forbidden thread.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::NoteKind;
use atomik_engine::ScheduledNote;
use atomik_testkit::no_alloc::{forbid_on_this_thread, worker_violations};
use common::patches::{fake_graph, notes_for, real_patch};
use common::*;
use std::time::Duration;

#[test]
fn multicore_render_allocates_on_no_thread() {
    let c = cfg(1, 4);
    let spec: Vec<_> = (0..24u8)
        .map(|k| (k, usize::from(k) * 7 + 3, f32::from(k) / 10.0 - 1.0))
        .collect();
    for threads in [1u16, 2, 3, 7] {
        // Previous engines are joined; std frees each exited thread's closure on that thread
        // (documented on `worker_violations`), so count per engine.
        let baseline = worker_violations();
        let (mut engine, mut handle) = engine_with(c, threads, Some(forbid_on_this_thread));
        let mut live = handle.live_notes().unwrap();
        for round in 0..3u64 {
            let fake = fake_graph(&c, &spec, [1, 5, 9, 30]);
            let real = real_patch(&cfg(1, 4), 4);
            for built in [fake, real] {
                for n in notes_for(&built.note_targets, 2000) {
                    handle.send_note(n).unwrap();
                    live.push(ScheduledNote {
                        kind: NoteKind::AllOff,
                        ..n
                    })
                    .unwrap();
                }
                handle.submit_plan(built.plan).unwrap();
                // Plan swaps, host blocks of every shape, idle gaps so workers park and wake.
                for block in [1usize, 64, 333] {
                    let ins = vec![vec![0.1; 999]];
                    render_blocks(&mut engine, &ins, 4, 999, block);
                    std::thread::sleep(Duration::from_millis(5 * round));
                }
                handle.collect_garbage();
            }
        }
        // Workers are still running here (they are joined when the engine drops).
        assert_eq!(worker_violations() - baseline, 0, "threads {threads}");
        // Teardown (joining workers) is non-RT: std's per-thread cleanup frees memory on the
        // exiting worker, and how much is platform-specific (1 per thread on Windows, more on
        // glibc/macOS). The next iteration's baseline is taken after this drop.
        drop(engine);
    }
}
