//! Multicore rendering: bit-identical output for any worker count,
//! worker lifecycle, and idle parking.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{BATCH, NoteKind, ParamId};
use atomik_engine::{EngineConfig, Graph, ScheduledNote, WORKER_SPIN_LIMIT, new_engine};
use atomik_testkit::fakes::{ConstantSource, Panicker, Passthrough};
use common::patches::{Built, NodeSpec, fake_graph, notes_for, real_patch, sixteen_synths};
use common::*;
use proptest::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const THREADS: [u16; 5] = [0, 1, 2, 3, 7];
const BLOCKS: [usize; 3] = [1, 64, 333];

fn noise(n: usize) -> Vec<f32> {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        })
        .collect()
}

/// Renders `built` with `threads` workers in host blocks of `block` (every render guarded).
fn run(c: EngineConfig, built: Built, threads: u16, block: usize, frames: usize) -> Vec<Vec<f32>> {
    let (mut engine, mut handle) = engine_with(c, threads, None);
    assert_eq!(engine.worker_count(), usize::from(threads));
    for n in notes_for(&built.note_targets, frames as u64) {
        handle.send_note(n).unwrap();
    }
    handle.submit_plan(built.plan).unwrap();
    let inputs = vec![noise(frames); usize::from(c.input_channels)];
    let out = render_blocks(
        &mut engine,
        &inputs,
        usize::from(c.output_channels),
        frames,
        block,
    );
    assert_eq!(handle.dropped_events(), 0);
    out
}

fn node_spec() -> impl Strategy<Value = NodeSpec> {
    (0u8..5, 0usize..64, -2.0f32..2.0)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    /// Random fake graphs with fan-out and parallel branches are bit-identical for every
    /// worker count and host block size.
    #[test]
    fn multicore_fake_graphs_are_bit_identical_to_single_threaded(
        spec in prop::collection::vec(node_spec(), 1..28),
        outs in prop::array::uniform4(0usize..64),
    ) {
        let c = cfg(1, 4);
        let frames = 1500;
        let reference = bits(&run(c, fake_graph(&c, &spec, outs), 0, 64, frames));
        for threads in THREADS {
            for block in BLOCKS {
                let got = bits(&run(c, fake_graph(&c, &spec, outs), threads, block, frames));
                prop_assert!(got == reference, "threads {} block {}", threads, block);
            }
        }
    }
}

#[test]
fn multicore_real_devices_are_bit_identical_to_single_threaded() {
    // with atomik-devices: synth -> delay -> mixer, plus a second synth into the mixer.
    let c = cfg(0, 2);
    let frames = 12_000;
    let reference = run(c, real_patch(&c, 2), 0, 64, frames);
    assert!(
        reference[0].iter().any(|s| s.abs() > 1e-3),
        "the patch must be audible"
    );
    let reference = bits(&reference);
    for threads in THREADS {
        for block in BLOCKS {
            let got = bits(&run(c, real_patch(&c, 2), threads, block, frames));
            assert!(got == reference, "threads {threads} block {block}");
        }
    }
}

#[test]
fn multicore_many_synths_are_bit_identical_with_param_changes_and_plan_swaps() {
    let c = cfg(0, 2);
    let render = |threads: u16| {
        let (mut engine, mut handle) = engine_with(c, threads, None);
        let mut out = vec![Vec::new(), Vec::new()];
        for round in 0..3u64 {
            let built = real_patch(&c, 6);
            let first = built.note_targets[0].0;
            for n in notes_for(&built.note_targets, 4000) {
                handle
                    .send_note(ScheduledNote {
                        frame: n.frame + round * 4000,
                        ..n
                    })
                    .unwrap();
            }
            handle.submit_plan(built.plan).unwrap();
            handle.set_param(first, ParamId(0), round as f32).unwrap();
            let o = render_blocks(&mut engine, &[], 2, 4000, 100);
            for (all, part) in out.iter_mut().zip(o) {
                all.extend(part);
            }
            handle.collect_garbage();
        }
        assert_eq!(handle.dropped_events(), 0);
        bits(&out)
    };
    let reference = render(0);
    for threads in [1, 4] {
        assert!(render(threads) == reference, "threads {threads}");
    }
}

#[test]
fn multicore_level_larger_than_worker_queues_still_renders_every_device() {
    // 600 independent sources in one level: more jobs than a worker queue holds.
    let c = cfg(0, 1);
    let build = || {
        let mut g = Graph::new(&c);
        let mut last = None;
        for k in 0..600 {
            last = Some(g.add_device(ConstantSource::boxed(k as f32 / 1000.0)));
        }
        let p = g.add_device(Passthrough::boxed(2.0));
        g.connect(
            ep(last.unwrap(), ConstantSource::AUDIO_OUT),
            ep(p, Passthrough::AUDIO_IN),
        )
        .unwrap();
        g.connect(ep(p, Passthrough::AUDIO_OUT), iface(&g, "out_1"))
            .unwrap();
        g.compile().unwrap()
    };
    let (mut engine, mut handle) = engine_with(c, 1, None);
    handle.submit_plan(build()).unwrap();
    let out = render_blocks(&mut engine, &[], 1, 256, 256);
    assert!(out[0][64..].iter().all(|s| (s - 1.198).abs() < 1e-6));
}

#[test]
fn threads_zero_spawns_no_worker() {
    static STARTED: AtomicUsize = AtomicUsize::new(0);
    fn count() {
        STARTED.fetch_add(1, Ordering::SeqCst);
    }
    let (engine, _handle) = engine_with(cfg(0, 2), 0, Some(count));
    assert_eq!(engine.worker_count(), 0);
    let (plain, _h) = new_engine(cfg(0, 2));
    assert_eq!(plain.worker_count(), 0);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(STARTED.load(Ordering::SeqCst), 0);
}

#[test]
fn every_worker_runs_on_thread_start_and_is_joined_on_drop() {
    static STARTED: AtomicUsize = AtomicUsize::new(0);
    static ALIVE: AtomicUsize = AtomicUsize::new(0);
    struct ExitProbe;
    impl Drop for ExitProbe {
        fn drop(&mut self) {
            ALIVE.fetch_sub(1, Ordering::SeqCst);
        }
    }
    thread_local!(static PROBE: ExitProbe = const { ExitProbe });
    fn start() {
        ALIVE.fetch_add(1, Ordering::SeqCst);
        PROBE.with(|_| ());
        STARTED.fetch_add(1, Ordering::SeqCst);
    }
    let (mut engine, mut handle) = engine_with(cfg(0, 2), 3, Some(start));
    assert_eq!(engine.worker_count(), 3);
    handle.submit_plan(real_patch(&cfg(0, 2), 3).plan).unwrap();
    render_blocks(&mut engine, &[], 2, 512, 128);
    for _ in 0..2000 {
        if STARTED.load(Ordering::SeqCst) == 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(STARTED.load(Ordering::SeqCst), 3);
    assert_eq!(ALIVE.load(Ordering::SeqCst), 3);
    drop(engine);
    assert_eq!(
        ALIVE.load(Ordering::SeqCst),
        0,
        "workers joined by Engine::drop"
    );
}

#[test]
fn idle_workers_park_and_stop_spinning() {
    // after rendering stops, workers spin a bounded number of times and then park.
    let c = cfg(0, 2);
    let (mut engine, mut handle) = engine_with(c, 4, None);
    handle.submit_plan(real_patch(&c, 4).plan).unwrap();
    render_blocks(&mut engine, &[], 2, 4096, 128);
    std::thread::sleep(Duration::from_millis(200));
    let settled = engine.worker_idle_spins();
    std::thread::sleep(Duration::from_secs(1));
    let after = engine.worker_idle_spins();
    assert!(settled > 0, "workers spin before parking");
    // Parked workers do not spin. A rare OS spurious wake-up costs at most one more bounded
    // spin round per worker; a non-parking worker would spin millions of times per second.
    assert!(
        after - settled <= WORKER_SPIN_LIMIT * 4,
        "workers spun {} times during 1 s idle",
        after - settled
    );
    // Rendering again wakes them and still produces sound.
    let out = render_blocks(&mut engine, &[], 2, 512, 128);
    assert!(out[0].iter().all(|s| s.is_finite()));
}

#[test]
fn render_levels_follow_dependencies() {
    let c = cfg(0, 2);
    let plan = real_patch(&c, 2).plan;
    let levels: Vec<Vec<u32>> = plan
        .levels()
        .map(|l| l.iter().map(|d| d.0).collect())
        .collect();
    // ids: synths 1, 2; delay 3; mixer 4. synth1 -> delay -> mixer <- synth2.
    assert_eq!(levels, vec![vec![1, 2], vec![3], vec![4]]);
}

// ---------------------------------------------------------------------------------------------
// A device that panics in `process`: a worker panic must surface on the audio thread, not hang.
// ---------------------------------------------------------------------------------------------

/// Two sources in one level, one of them the `Panicker`; `panicker_first` picks which runs on
/// the audio thread. Renders without the allocation guard (the panic path allocates by design)
/// and returns the output channels and the fault.
fn render_with_panicker(
    threads: u16,
    panicker_first: bool,
) -> (
    Vec<Vec<f32>>,
    Option<atomik_engine::DeviceInstanceId>,
    usize,
) {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let (steady, bad);
    if panicker_first {
        bad = g.add_device(Panicker::boxed());
        steady = g.add_device(ConstantSource::boxed(0.5));
    } else {
        steady = g.add_device(ConstantSource::boxed(0.5));
        bad = g.add_device(Panicker::boxed());
    }
    g.connect(ep(steady, ConstantSource::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    g.connect(ep(bad, Panicker::AUDIO_OUT), iface(&g, "out_2"))
        .unwrap();
    let (mut engine, mut handle) = engine_with(c, threads, None);
    assert_eq!(engine.worker_count(), usize::from(threads));
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let mut left = vec![0.0f32; BATCH * 4];
    let mut right = vec![0.0f32; BATCH * 4];
    engine.render(&[], &mut [&mut left[..], &mut right[..]]);
    assert_eq!(engine.fault(), handle.fault());
    assert_eq!(engine.fault(), Some(bad));
    (vec![left, right], engine.fault(), BATCH)
}

#[test]
fn a_panicking_device_is_contained() {
    for threads in [0u16, 3] {
        for panicker_first in [false, true] {
            let (out, fault, latency) = render_with_panicker(threads, panicker_first);
            assert!(fault.is_some(), "{threads} workers");
            // The engine kept rendering: the steady source sounds once the FIFO latency has
            // passed, and the faulted device is silent for every batch.
            assert!(
                out[0][latency..].iter().all(|s| (s - 0.5).abs() < 1e-7),
                "{threads} workers, panicker first: {panicker_first}"
            );
            assert!(out[1].iter().all(|s| *s == 0.0));
        }
    }
}

#[test]
fn a_faulted_device_stays_silent_and_the_engine_keeps_rendering() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    g.add_device(ConstantSource::boxed(0.0));
    g.add_device(Panicker::boxed());
    let (mut engine, mut handle) = engine_with(c, 3, None);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    assert_eq!(handle.fault(), None);
    let mut out = vec![0.0f32; BATCH];
    for _ in 0..50 {
        engine.render(&[], &mut [&mut out[..]]);
    }
    assert!(engine.fault().is_some());
    assert_eq!(handle.dropped_events(), 0);
}

// ---------------------------------------------------------------------------------------------
// Benchmark (ignored): real-time factor for 0 vs 4 workers on a 16-synth patch.
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
#[allow(clippy::disallowed_methods)] // Justification: benchmark timing only.
fn bench_real_time_factor_16_synths() {
    let c = cfg(0, 2);
    let seconds = 10usize;
    let frames = 48_000 * seconds;
    let block = 128;
    for threads in [0u16, 1, 2, 4, 8] {
        let built = sixteen_synths(&c);
        let (mut engine, mut handle) = engine_with(c, threads, None);
        for (k, &(device, socket)) in built.note_targets.iter().enumerate() {
            for v in 0..6u8 {
                handle
                    .send_note(ScheduledNote {
                        frame: 0,
                        device,
                        socket,
                        kind: NoteKind::On {
                            key: 36 + (k as u8) + v * 7,
                            velocity: 0.7,
                        },
                    })
                    .unwrap();
            }
        }
        handle.submit_plan(built.plan).unwrap();
        let mut l = vec![0.0f32; block];
        let mut r = vec![0.0f32; block];
        let t0 = std::time::Instant::now();
        for _ in 0..frames / block {
            engine.render(&[], &mut [&mut l[..], &mut r[..]]);
        }
        let took = t0.elapsed().as_secs_f64();
        println!(
            "threads {threads}: {seconds} s of audio in {took:.3} s -> real-time factor {:.1}x",
            seconds as f64 / took
        );
    }
}
