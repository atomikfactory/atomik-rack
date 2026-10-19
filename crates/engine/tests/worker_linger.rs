//! Worker wake-up mitigation: `EngineOptions::worker_linger_spins` keeps idle workers spinning
//! longer before they park, so a worker that finished one callback's batches is still awake
//! for the next one. Idle parking must still hold after a longer idle; output stays
//! bit-identical and rendering never allocates.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::NoteKind;
use atomik_engine::{
    Engine, EngineHandle, EngineOptions, ScheduledNote, WORKER_SPIN_LIMIT, WorkerConfig,
    new_engine_with_options,
};
use common::patches::{notes_for, real_patch};
use common::*;
use std::time::Duration;

fn engine_lingering(threads: u16, linger: u64) -> (Engine, EngineHandle) {
    new_engine_with_options(
        cfg(0, 2),
        EngineOptions {
            workers: WorkerConfig {
                threads,
                on_thread_start: None,
            },
            worker_linger_spins: linger,
        },
    )
}

#[test]
fn default_options_keep_the_short_spin_limit() {
    assert_eq!(EngineOptions::default().worker_linger_spins, 0);
    let (engine, _) = engine_lingering(1, 0);
    assert_eq!(engine.worker_linger_spins(), WORKER_SPIN_LIMIT);
    let (engine, _) = engine_lingering(1, 123_456);
    assert_eq!(engine.worker_linger_spins(), 123_456);
}

#[test]
fn lingering_workers_stay_awake_between_close_renders() {
    // A linger far longer than the gaps: once started, workers never park while renders keep
    // coming (they only park after the long linger, or at shutdown).
    let c = cfg(0, 2);
    let (mut engine, mut handle) = engine_lingering(2, u64::MAX);
    handle.submit_plan(real_patch(&c, 4).plan).unwrap();
    for _ in 0..5 {
        render_blocks(&mut engine, &[], 2, 512, 128);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(engine.worker_parks(), 0, "no worker parked between renders");
    drop(engine); // shutdown still stops spinning workers
}

#[test]
fn short_linger_parks_between_renders() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = engine_lingering(2, 1);
    handle.submit_plan(real_patch(&c, 4).plan).unwrap();
    for _ in 0..3 {
        render_blocks(&mut engine, &[], 2, 512, 128);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(engine.worker_parks() > 0);
}

#[test]
fn lingering_workers_still_park_after_a_longer_idle() {
    // with the linger the player uses: after rendering stops the workers spin at most the
    // linger, then park and stop spinning.
    let linger = WORKER_SPIN_LIMIT * 25;
    let c = cfg(0, 2);
    let (mut engine, mut handle) = engine_lingering(4, linger);
    handle.submit_plan(real_patch(&c, 4).plan).unwrap();
    render_blocks(&mut engine, &[], 2, 4096, 128);
    std::thread::sleep(Duration::from_millis(500));
    let settled = engine.worker_idle_spins();
    let parks = engine.worker_parks();
    std::thread::sleep(Duration::from_secs(1));
    let after = engine.worker_idle_spins();
    assert!(settled > 0, "workers spin before parking");
    assert!(parks >= 4, "every worker parked: {parks}");
    assert!(
        after - settled <= linger * 4,
        "workers spun {} times during 1 s idle",
        after - settled
    );
    let out = render_blocks(&mut engine, &[], 2, 512, 128);
    assert!(out[0].iter().all(|s| s.is_finite()));
}

#[test]
fn linger_does_not_change_the_output() {
    // bit-identical to the single-threaded engine whatever the linger.
    let c = cfg(0, 2);
    let render = |threads: u16, linger: u64| {
        let built = real_patch(&c, 4);
        let (mut engine, mut handle) = engine_lingering(threads, linger);
        for n in notes_for(&built.note_targets, 8192) {
            handle.send_note(n).unwrap();
        }
        handle.submit_plan(built.plan).unwrap();
        bits(&render_blocks(&mut engine, &[], 2, 8192, 128))
    };
    let reference = render(0, 0);
    assert_eq!(render(3, WORKER_SPIN_LIMIT * 25), reference);
    assert_eq!(render(3, u64::MAX), reference);
}

// ---------------------------------------------------------------------------------------------
// Benchmark (ignored): renders paced like a 128-frame / 48 kHz callback, per linger.
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
#[allow(clippy::disallowed_methods)] // Justification: benchmark timing only.
fn bench_paced_callbacks_per_linger() {
    use std::time::Instant;
    let c = cfg(0, 2);
    let block = 128usize;
    let period = Duration::from_secs_f64(block as f64 / 48_000.0);
    let callbacks = 3750usize; // 10 s
    // Cost of one spin, for reading barrier spins as time.
    let t0 = Instant::now();
    for _ in 0..10_000_000 {
        std::hint::spin_loop();
    }
    println!(
        "spin_loop: {:.1} ns per spin",
        t0.elapsed().as_secs_f64() * 1e9 / 1e7
    );
    for (threads, linger) in [
        (0u16, 0u64),
        (4, 0),
        (4, WORKER_SPIN_LIMIT * 5),
        (4, WORKER_SPIN_LIMIT * 25),
    ] {
        let built = real_patch(&c, 4);
        let (mut engine, mut handle) = engine_lingering(threads, linger);
        for (k, &(device, socket)) in built.note_targets.iter().enumerate() {
            for v in 0..6u8 {
                handle
                    .send_note(ScheduledNote {
                        frame: 0,
                        device,
                        socket,
                        kind: NoteKind::On {
                            key: 36 + k as u8 + v * 7,
                            velocity: 0.7,
                        },
                    })
                    .unwrap();
            }
        }
        handle.submit_plan(built.plan).unwrap();
        let mut l = vec![0.0f32; block];
        let mut r = vec![0.0f32; block];
        let mut times = Vec::with_capacity(callbacks);
        let mut max_spins = 0u64;
        let mut next = Instant::now() + period;
        for _ in 0..callbacks {
            // Wait like an audio thread between callbacks: sleep, then spin to the deadline.
            while let Some(left) = next.checked_duration_since(Instant::now()) {
                if left > Duration::from_millis(2) {
                    std::thread::sleep(Duration::from_millis(1));
                } else {
                    std::hint::spin_loop();
                }
            }
            next += period;
            let start = Instant::now();
            engine.render(&[], &mut [&mut l[..], &mut r[..]]);
            times.push(start.elapsed());
            max_spins = max_spins.max(engine.take_max_barrier_spins());
        }
        times.sort();
        let pct = |p: f64| times[((times.len() - 1) as f64 * p) as usize].as_secs_f64() * 1e6;
        let load = |us: f64| us / (period.as_secs_f64() * 1e6) * 100.0;
        let over_half = times.iter().filter(|t| **t > period / 2).count();
        println!(
            "threads {threads} linger {linger:>7}: render p50 {:6.1} us ({:4.1}%), p99 {:6.1} us, \
             p99.9 {:6.1} us, max {:7.1} us ({:5.1}%), >50% load: {over_half}, \
             max barrier spins {max_spins}, parks {}",
            pct(0.5),
            load(pct(0.5)),
            pct(0.99),
            pct(0.999),
            pct(1.0),
            load(pct(1.0)),
            engine.worker_parks()
        );
    }
}
