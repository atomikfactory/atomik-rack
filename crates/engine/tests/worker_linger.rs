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
    let (mut engine, handle) = lingering_engine(threads, linger, false);
    // These tests are about the workers' spin and park: every level is handed over.
    engine.set_gate_mode(atomik_engine::GateMode::ForceDispatch);
    (engine, handle)
}

fn lingering_engine(threads: u16, linger: u64, realtime: bool) -> (Engine, EngineHandle) {
    new_engine_with_options(
        cfg(0, 2),
        EngineOptions {
            workers: WorkerConfig {
                threads,
                on_thread_start: None,
            },
            worker_linger_spins: linger,
            realtime,
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

/// Waits (generously: the test never depends on the speed of the machine) until `done` holds.
fn wait_until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..4000 {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out: {what}");
}

#[test]
fn workers_park_between_callbacks_even_with_an_endless_linger() {
    // The linger bounds the spinning inside a callback only: once the render call returns,
    // no worker spins on, whatever the linger.
    let c = cfg(0, 2);
    let (mut engine, mut handle) = engine_lingering(2, u64::MAX);
    handle.submit_plan(real_patch(&c, 4).plan).unwrap();
    render_blocks(&mut engine, &[], 2, 4096, 128);
    // The workers park (and flush their spin counts) a moment after the last render call
    // returns: wait until both counters stand still. Workers that spun on would never let them.
    wait_until("the workers came to rest", || {
        let before = (engine.worker_idle_spins(), engine.worker_parks());
        std::thread::sleep(Duration::from_millis(100));
        before.1 >= 2 && before == (engine.worker_idle_spins(), engine.worker_parks())
    });
    let settled = engine.worker_idle_spins();
    let parks = engine.worker_parks();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        engine.worker_idle_spins(),
        settled,
        "no spinning while idle"
    );
    assert_eq!(engine.worker_parks(), parks, "the workers stay parked");
    // The next callback wakes them again and still renders.
    let out = render_blocks(&mut engine, &[], 2, 512, 128);
    assert!(out[0].iter().all(|s| s.is_finite()));
    drop(engine); // shutdown still wakes parked workers
}

#[test]
fn workers_spin_while_a_callback_runs() {
    // Inside a callback the linger applies as before: over many callbacks the workers spin
    // (the count is flushed when they park at the end of each one).
    let c = cfg(0, 2);
    let (mut engine, mut handle) = engine_lingering(2, u64::MAX);
    handle.submit_plan(real_patch(&c, 4).plan).unwrap();
    for _ in 0..20 {
        render_blocks(&mut engine, &[], 2, 4096, 128);
    }
    // The spins are counted when a worker parks, and the workers have parked before (at their
    // start), so a park count says nothing here: wait for the count itself, which the park
    // after the last callback brings.
    wait_until("workers spun inside callbacks", || {
        engine.worker_idle_spins() > 0
    });
}

#[test]
fn a_forced_flag_keeps_the_old_spin_between_callbacks() {
    // The benchmark switch: with the flag forced, workers do not park between close renders.
    let c = cfg(0, 2);
    let (mut engine, mut handle) = engine_lingering(2, u64::MAX);
    engine.set_workers_always_active(true);
    handle.submit_plan(real_patch(&c, 4).plan).unwrap();
    // The workers may have parked once before the switch (they start with the flag down).
    render_blocks(&mut engine, &[], 2, 512, 128);
    let parked = engine.worker_parks();
    assert!(parked <= 2);
    for _ in 0..5 {
        render_blocks(&mut engine, &[], 2, 512, 128);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        engine.worker_parks(),
        parked,
        "no worker parked between renders"
    );
    drop(engine);
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
    assert_eq!(render(1, u64::MAX), reference);
    // The forced flag of the benchmark changes nothing either.
    let forced = |threads: u16| {
        let built = real_patch(&c, 4);
        let (mut engine, mut handle) = engine_lingering(threads, WORKER_SPIN_LIMIT);
        engine.set_workers_always_active(true);
        for n in notes_for(&built.note_targets, 8192) {
            handle.send_note(n).unwrap();
        }
        handle.submit_plan(built.plan).unwrap();
        bits(&render_blocks(&mut engine, &[], 2, 8192, 128))
    };
    assert_eq!(forced(3), reference);
}

// ---------------------------------------------------------------------------------------------
// Benchmark (ignored): renders paced like a 48 kHz audio callback, per worker count and linger,
// per patch: with no workers, with every level of two or more devices handed over ("dispatch"),
// and with the load rule of a live engine deciding ("live"). The share column is the median
// callback of the no-workers row as a percentage of the callback period, that is the load the
// audio thread alone carries; the rule enters at 30 % and leaves under 20 %.
// ---------------------------------------------------------------------------------------------

/// CPU seconds this process has used so far (all threads), or `None` where the benchmark has no
/// way to read it. Windows asks the shell, Linux reads `/proc`; the cost of asking is not ours.
fn process_cpu_seconds() -> Option<f64> {
    #[cfg(windows)]
    {
        let script = format!(
            "(Get-Process -Id {}).TotalProcessorTime.TotalSeconds",
            std::process::id()
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .replace(',', ".")
            .parse()
            .ok()
    }
    #[cfg(target_os = "linux")]
    {
        // Fields 14 and 15 (utime, stime) in clock ticks, 100 per second on every common kernel.
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        let rest = stat.rsplit_once(')')?.1;
        let mut f = rest.split_whitespace().skip(11);
        let (u, s): (f64, f64) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?);
        Some((u + s) / 100.0)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
#[allow(clippy::disallowed_methods)] // Justification: benchmark timing only.
fn bench_paced_callbacks_per_linger() {
    use std::time::Instant;
    let c = cfg(0, 2);
    // Cost of one spin, for reading barrier spins as time.
    let t0 = Instant::now();
    for _ in 0..10_000_000 {
        std::hint::spin_loop();
    }
    println!(
        "spin_loop: {:.1} ns per spin",
        t0.elapsed().as_secs_f64() * 1e9 / 1e7
    );
    println!(
        "patch    frames workers mode     | callback us: p50   p99   max | share % | busy | idle spins          | parks | cpu % of a core"
    );
    // The synth count and the voices played on each: light, the patch of the other rows of
    // `perf/BASELINE.md`, and a heavy one.
    for (label, synths, voices) in [("light", 1usize, 1u8), ("existing", 4, 6), ("heavy", 6, 16)] {
        for block in [128usize, 480] {
            let period = Duration::from_secs_f64(block as f64 / 48_000.0);
            let callbacks = (10.0 / period.as_secs_f64()) as usize; // 10 s
            let mut share = f64::NAN;
            for (threads, mode) in [(0u16, "inline"), (4, "dispatch"), (4, "live")] {
                let built = real_patch(&c, synths);
                let (mut engine, mut handle) = match mode {
                    "live" => lingering_engine(threads, WORKER_SPIN_LIMIT * 25, true),
                    _ => engine_lingering(threads, WORKER_SPIN_LIMIT * 25),
                };
                for (k, &(device, socket)) in built.note_targets.iter().enumerate() {
                    for v in 0..voices {
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
                let cpu0 = process_cpu_seconds();
                let wall = Instant::now();
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
                let wall = wall.elapsed().as_secs_f64();
                let cpu = match (cpu0, process_cpu_seconds()) {
                    (Some(a), Some(b)) => format!("{:6.1}", (b - a) / wall * 100.0),
                    _ => "   n/a".to_string(),
                };
                times.sort();
                let pct =
                    |p: f64| times[((times.len() - 1) as f64 * p) as usize].as_secs_f64() * 1e6;
                if threads == 0 {
                    share = pct(0.5) / (period.as_secs_f64() * 1e6) * 100.0;
                }
                println!(
                    "{label:<8} {block:>6} {threads:>7} {mode:<8} | {:>16.1} {:>5.1} {:>6.1} | {share:>7.1} | {:>4} | {:>19} | {:>5} | {cpu}",
                    pct(0.5),
                    pct(0.99),
                    pct(1.0),
                    engine.workers_busy(),
                    engine.worker_idle_spins(),
                    engine.worker_parks(),
                );
            }
        }
    }
}

#[test]
fn the_handle_reads_the_worker_counters_the_engine_keeps() {
    let c = cfg(0, 2);
    let (engine, handle) = engine_lingering(2, 1);
    let stats = handle.worker_stats();
    assert_eq!((stats.threads, stats.linger), (2, 1));
    let (mut engine, mut handle) = (engine, handle);
    handle.submit_plan(real_patch(&c, 4).plan).unwrap();
    for _ in 0..3 {
        render_blocks(&mut engine, &[], 2, 512, 128);
        std::thread::sleep(Duration::from_millis(20));
    }
    let stats = handle.worker_stats();
    assert_eq!(stats.parks, engine.worker_parks());
    assert!(stats.parks > 0);
    assert_eq!(stats.idle_spins, engine.worker_idle_spins());
    let (_, none) = engine_lingering(0, 0);
    assert_eq!(none.worker_stats(), atomik_engine::WorkerStats::default());
}
