//! The stress projects play with no heap operation per audio block: the stress song (eight
//! synths through the console, four delays on the returns, automation) and the stress rack
//! (a hundred devices, three hundred cables) render ten seconds of audio at 48 kHz in blocks of
//! 256 frames with the worker pool on. After a short warm-up every block is rendered under
//! `assert_no_alloc` on the audio thread, and every worker is marked `forbid_on_this_thread`,
//! so a heap operation on a worker after the warm-up fails the test as well. Own binary: the
//! worker violation counter is process-global.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{
    EngineOptions, GateMode, OfflineJob, WorkerConfig, compile_song, new_engine_with_options,
    render_offline_song,
};
use atomik_model::RackDoc;
use atomik_model::stress::{STRESS_CABLES, STRESS_DEVICES, stress_doc, stress_song_doc};
use atomik_render::build::build_with_devices;
use atomik_render::project::{bounce_plan, console_options, expanded_ids, song_input_with_warps};
use atomik_render::warp::WarpStore;
use atomik_render::{default_registry, flat};
use atomik_testkit::no_alloc::{assert_no_alloc, forbid_on_this_thread, worker_violations};
use std::cell::Cell;
use std::collections::BTreeMap;

/// Frames per host block.
const BLOCK: usize = 256;
/// Blocks rendered before anything is measured: every lazily made buffer and cache exists.
const WARM_BLOCKS: usize = 64;
/// Blocks measured: ten seconds at 48 kHz.
const MEASURED_BLOCKS: usize = 10 * 48_000 / BLOCK;

/// Renders `doc` with `workers` extra threads and returns the audio of the measured stretch.
/// Panics (inside `assert_no_alloc`) when a block allocated on this thread; the number of heap
/// operations the workers made after the warm-up is returned.
fn measure(doc: &RackDoc, workers: u16) -> (Vec<Vec<f32>>, u64) {
    let registry = default_registry();
    let expanded = doc.expand(&registry);
    let (patch, _) = flat::expanded_patch(doc, &expanded);
    let built = build_with_devices(
        &patch,
        &registry,
        &console_options(doc, &registry),
        BTreeMap::new(),
    )
    .expect("the stress project builds");
    let ids = expanded_ids(&expanded);
    let input = song_input_with_warps(
        doc,
        &registry,
        |k| ids.get(k).copied(),
        &BTreeMap::new(),
        &WarpStore::new(),
    );
    let snapshot = compile_song(&input).expect("the song compiles");
    let plan = bounce_plan(doc, &snapshot, None);
    let (mut engine, mut handle) = new_engine_with_options(
        built.config,
        EngineOptions {
            workers: WorkerConfig {
                threads: workers,
                on_thread_start: Some(forbid_on_this_thread),
            },
            worker_linger_spins: 0,
            realtime: false,
        },
    );
    engine.set_gate_mode(GateMode::ForceDispatch);
    handle.submit_plan(built.plan).expect("plan queued");
    for command in plan.setup {
        handle.transport(command).expect("transport queued");
    }
    let job = OfflineJob {
        frames: (WARM_BLOCKS + MEASURED_BLOCKS) * BLOCK,
        host_block: BLOCK,
        notes: &[],
        params: &[],
        inputs: &[],
    };
    let blocks = Cell::new(0usize);
    let baseline = Cell::new(0u64);
    let out = render_offline_song(&mut engine, &mut handle, &job, snapshot, Vec::new(), |f| {
        let n = blocks.get();
        blocks.set(n + 1);
        if n < WARM_BLOCKS {
            f();
        } else {
            if n == WARM_BLOCKS {
                baseline.set(worker_violations());
            }
            assert_no_alloc(f);
        }
    });
    let worker_heap_ops = worker_violations() - baseline.get();
    // The render also covers the plan's latency, a block more or less.
    assert!(blocks.get() >= WARM_BLOCKS + MEASURED_BLOCKS);
    drop(engine);
    (out, worker_heap_ops)
}

fn audible(out: &[Vec<f32>]) -> bool {
    out.iter()
        .flatten()
        .skip(WARM_BLOCKS * BLOCK)
        .any(|s| s.abs() > 1e-4)
}

/// One test for both projects: the worker counter is process-global, so two tests running side
/// by side would count each other's workers shutting down.
#[test]
fn the_stress_projects_render_ten_seconds_without_a_heap_operation() {
    let registry = default_registry();
    let song = stress_song_doc(&registry);
    let rack = stress_doc(&registry, STRESS_DEVICES, STRESS_CABLES);
    for (name, doc) in [("song", &song), ("rack", &rack)] {
        for workers in [0u16, 2] {
            let (out, worker_heap_ops) = measure(doc, workers);
            if name == "song" {
                assert!(audible(&out), "the song is audible, workers {workers}");
            }
            assert_eq!(worker_heap_ops, 0, "{name}, workers {workers}");
        }
    }
}
