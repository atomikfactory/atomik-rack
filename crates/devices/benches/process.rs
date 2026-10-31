//! Cost of one device batch for every built-in device: each device is prepared at 48 kHz, every
//! connected input gets deterministic noise, notes inputs get a held three-note chord, and 256
//! batches (a third of a second of audio) are processed per timed run. The cost per batch is the
//! median run divided by 256. Run: `cargo bench -p atomik-devices --bench process`; a filter
//! argument selects devices (`-- poly-synth`). `ATOMIK_BENCH_ONCE` runs each device once.
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_device_sdk::{Registry, SocketKind};
use atomik_testkit::bench::{Recorder, SAMPLES, once_mode, time_once};
use atomik_testkit::harness::DeviceHarness;
use criterion::Criterion;
use std::time::Duration;

const BATCHES: usize = 256;
/// Types that need host data (children, audio assets or a plugin) and cannot run bare.
const SKIPPED: [&str; 2] = ["atomik.container", "atomik.audio-track"];

fn noise(seed: &mut u32) -> [f32; BATCH] {
    let mut b = [0.0f32; BATCH];
    for s in &mut b {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *s = (*seed >> 8) as f32 / (1u32 << 24) as f32 * 0.5 - 0.25;
    }
    b
}

/// A device of `type_id` with every input connected and a chord held on its notes inputs.
fn rig(registry: &Registry, type_id: &str) -> DeviceHarness {
    let factory = registry.get(type_id).expect("registered type");
    let mut h = DeviceHarness::new((factory.create)(), 48_000.0);
    let mut seed = 7;
    for s in factory.descriptor.sockets {
        if !s.direction.is_input() {
            continue;
        }
        match s.kind {
            SocketKind::Audio | SocketKind::Cv => *h.input_mut(s.key) = noise(&mut seed),
            SocketKind::Notes => {
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
        }
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
            let mut h = rig(&registry, id);
            println!("{id}: once {:.6} s", time_once(|| run(&mut h)));
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
        let mut h = rig(&registry, id);
        let rec = Recorder::new();
        c.bench_function(&name, |b| b.iter_custom(rec.runner(|| run(&mut h))));
        rec.report(&name);
    }
    c.final_summary();
}
