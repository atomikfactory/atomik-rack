//! Cost of one device batch for every built-in device: each device is prepared at 48 kHz, every
//! connected input gets deterministic noise, notes inputs get a held three-note chord, and 256
//! batches (a third of a second of audio) are processed per timed run. The cost per batch is the
//! median run divided by 256. Run: `cargo bench -p atomik-devices --bench process`; a filter
//! argument selects devices (`-- poly-synth`). `ATOMIK_BENCH_ONCE` runs each device once. The
//! built-in instruments also run an `idle` row (`device/<name>/idle/256batches`): the same rig
//! without notes, what an instrument costs while nothing is played. The mastering device and the
//! phase meter also run an `unobserved` row (`device/<name>/unobserved/256batches`): the same rig
//! with the observed flag cleared, what they cost while no panel shows them.
//! The convolution reverb also runs a `convolver-ir` row (`device/convolver-ir/256batches`): the rig with a
//! 3 s synthetic impulse response loaded and built, which the other rows never have; a second line
//! (`device/convolver-ir/batch-max`) gives the dearest single batch of 4096 (the tail of every partition is
//! summed in one batch out of four).
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_device_sdk::{
    LoopMode, Registry, SampleBuffer, SampleMap, SampleMapRef, SocketKind, TriggerMode, Zone,
    ZoneExt,
};
use atomik_testkit::bench::{Recorder, SAMPLES, once_mode, time_once};
use atomik_testkit::harness::DeviceHarness;
use criterion::Criterion;
use std::sync::Arc;
use std::time::{Duration, Instant};

const BATCHES: usize = 256;
/// Types that need host data (children, audio assets or a plugin) and cannot run bare.
const SKIPPED: [&str; 2] = ["atomik.container", "atomik.audio-track"];
/// The devices whose display-only metering stops while no panel shows them (the observed flag).
const OBSERVED: [&str; 2] = ["atomik.mastering", "atomik.phase-meter"];
/// The instruments that also get an idle row.
const INSTRUMENTS: [&str; 6] = [
    "atomik.poly-synth",
    "atomik.fm-synth",
    "atomik.wavetable-synth",
    "atomik.sampler",
    "atomik.granular",
    "atomik.organ",
];

fn noise(seed: &mut u32) -> [f32; BATCH] {
    let mut b = [0.0f32; BATCH];
    for s in &mut b {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *s = (*seed >> 8) as f32 / (1u32 << 24) as f32 * 0.5 - 0.25;
    }
    b
}

/// A device of `type_id` with every input connected and, with `chord`, a chord held on its
/// notes inputs.
fn rig(registry: &Registry, type_id: &str, chord: bool) -> DeviceHarness {
    let factory = registry.factory(type_id).expect("registered type");
    let mut h = DeviceHarness::new((factory.create)(), 48_000.0);
    let mut seed = 7;
    for s in factory.descriptor.sockets {
        if !s.direction.is_input() {
            continue;
        }
        match s.kind {
            SocketKind::Audio | SocketKind::Cv => *h.input_mut(s.key) = noise(&mut seed),
            SocketKind::Notes if chord => {
                for key in [48u8, 55, 64] {
                    h.push_note(
                        s.key,
                        NoteEvent {
                            offset: 0,
                            kind: NoteKind::On { key, velocity: 0.7 },
                        },
                    );
                }
            }
            SocketKind::Notes => {}
        }
    }
    h
}

/// The rig of a device that carries the observed flag, with the flag cleared.
fn unobserved(registry: &Registry, type_id: &str) -> DeviceHarness {
    let h = rig(registry, type_id, true);
    h.device()
        .scope_tap()
        .expect("a tap that carries the flag")
        .set_observed(false);
    h
}

/// Length of the synthetic impulse response of the `convolver-ir` row, in seconds.
const IR_SECONDS: f32 = 3.0;

/// A map whose first zone is `seconds` of deterministic noise with an exponential decay, mono.
fn impulse(seconds: f32) -> SampleMapRef {
    let frames = (seconds * 48_000.0) as usize;
    let mut seed = 99;
    let ir: Vec<f32> = noise_stream(&mut seed, frames)
        .into_iter()
        .enumerate()
        .map(|(i, v)| v * (-6.9 * i as f32 / frames as f32).exp())
        .collect();
    let buffer = Arc::new(SampleBuffer::new(vec![ir].into()).expect("a buffer"));
    let zone = Zone {
        buffer: 0,
        key_lo: 0,
        key_hi: 127,
        vel_lo: 1,
        vel_hi: 127,
        root_key: 60,
        transpose: 0,
        tune_cents: 0.0,
        gain_db: 0.0,
        pan: 0.0,
        start: 0,
        end: frames as u64,
        loop_mode: LoopMode::Off,
        loop_start: 0,
        loop_end: 0,
        crossfade: 0,
        trigger: TriggerMode::OneShot,
        reverse: false,
        ext: ZoneExt::NONE,
    };
    Arc::new(SampleMap::new(vec![buffer], vec![zone]).expect("a map"))
}

/// `n` noise samples in -0.25..0.25.
fn noise_stream(seed: &mut u32, n: usize) -> Vec<f32> {
    (0..n.div_ceil(BATCH))
        .flat_map(|_| noise(seed))
        .take(n)
        .collect()
}

/// The convolution reverb with its impulse response loaded and the build finished (the partitions
/// are transformed a few per batch, 3 s take about 70 batches).
fn convolver_ir(registry: &Registry) -> DeviceHarness {
    let mut h = rig(registry, "atomik.convolver", true);
    drop(h.device_mut().swap_samples(Some(impulse(IR_SECONDS))));
    for _ in 0..192 {
        h.process_unguarded();
    }
    h
}

fn run(h: &mut DeviceHarness) -> f32 {
    for _ in 0..BATCHES {
        h.process_unguarded();
    }
    h.device().descriptor().sockets.len() as f32
}

fn main() {
    let mut registry = Registry::new();
    atomik_devices::register_all(&mut registry).expect("built-in devices register");
    let ids: Vec<&'static str> = registry
        .type_ids()
        .filter(|t| !SKIPPED.contains(t))
        .collect();
    if once_mode() {
        for id in ids {
            let mut h = rig(&registry, id, true);
            println!("{id}: once {:.6} s", time_once(|| run(&mut h)));
            if id == "atomik.convolver" {
                let mut h = convolver_ir(&registry);
                println!("{id}: ir once {:.6} s", time_once(|| run(&mut h)));
            }
            if INSTRUMENTS.contains(&id) {
                let mut h = rig(&registry, id, false);
                println!("{id}: idle once {:.6} s", time_once(|| run(&mut h)));
            }
            if OBSERVED.contains(&id) {
                let mut h = unobserved(&registry, id);
                println!("{id}: unobserved once {:.6} s", time_once(|| run(&mut h)));
            }
        }
        return;
    }
    let mut c = Criterion::default()
        .sample_size(SAMPLES)
        .warm_up_time(Duration::from_millis(30))
        .measurement_time(Duration::from_millis(300))
        .configure_from_args();
    for id in ids {
        let name = format!(
            "device/{}/{BATCHES}batches",
            id.trim_start_matches("atomik.")
        );
        let mut h = rig(&registry, id, true);
        let rec = Recorder::new();
        c.bench_function(&name, |b| b.iter_custom(rec.runner(|| run(&mut h))));
        rec.report(&name);
        if INSTRUMENTS.contains(&id) {
            let name = format!(
                "device/{}/idle/{BATCHES}batches",
                id.trim_start_matches("atomik.")
            );
            let mut h = rig(&registry, id, false);
            let rec = Recorder::new();
            c.bench_function(&name, |b| b.iter_custom(rec.runner(|| run(&mut h))));
            rec.report(&name);
        }
        if id == "atomik.convolver" {
            let name = format!("device/convolver-ir/{BATCHES}batches");
            let mut h = convolver_ir(&registry);
            let rec = Recorder::new();
            c.bench_function(&name, |b| b.iter_custom(rec.runner(|| run(&mut h))));
            rec.report(&name);
            // The dearest single batch (the tail of every partition is summed in one batch of
            // four): the highest of 4096 batches, five times, and the median of the five.
            if rec.summary().is_some() {
                let mut worst: Vec<f64> = (0..5)
                    .map(|_| {
                        (0..4_096)
                            .map(|_| {
                                let start = Instant::now();
                                h.process_unguarded();
                                start.elapsed()
                            })
                            .max()
                            .unwrap_or_default()
                            .as_secs_f64()
                            * 1e6
                    })
                    .collect();
                worst.sort_by(f64::total_cmp);
                println!(
                    "device/convolver-ir/batch-max: median {:.1} us  highest {:.1} us  (5 x 4096 batches)",
                    worst[2], worst[4]
                );
            }
        }
        if OBSERVED.contains(&id) {
            let name = format!(
                "device/{}/unobserved/{BATCHES}batches",
                id.trim_start_matches("atomik.")
            );
            let mut h = unobserved(&registry, id);
            let rec = Recorder::new();
            c.bench_function(&name, |b| b.iter_custom(rec.runner(|| run(&mut h))));
            rec.report(&name);
        }
    }
    c.final_summary();
}
