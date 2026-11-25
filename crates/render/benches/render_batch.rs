//! Bounce speed of generated projects: how long the offline renderer takes (build and render)
//! for racks, songs, consoles and automation-heavy songs, at several worker counts and host
//! blocks. Run one command per crate:
//!
//! ```text
//! cargo bench -p atomik-render --bench render_batch
//! ATOMIK_BENCH_FULL=1 cargo bench -p atomik-render --bench render_batch   # workers 0,1,2,4 x blocks 64,256,1024
//! ```
//!
//! Each case prints criterion's report and a line `median .. p99 ..` over its timed runs. With
//! `ATOMIK_BENCH_ONCE` set every scenario renders exactly once (for an instruction counter).
//! A filter argument selects scenarios: `cargo bench -p atomik-render --bench render_batch -- song`.
//!
//! Scenarios: `rack` (100 devices, 300 cables), `song` and `console-8` (the 8-track stress song
//! with 4 FX returns), `console-64` (64 strips, gate and compressor on), `silent-strips-64` (64 strips that
//! never play, filters, EQ, compressors and two sends on), `lanes-200` and
//! `lanes-2000` (automation lanes), `large-song` (100 tracks, 500 clips, 2,000 notes).
//! Every scenario bounces `BARS` bars plus the release tail, so a run is a few seconds of audio.
//! The `idle` rows (`idle/stereo-delay-x20/skip-off` and `skip-on`, and one pair for each other effect or
//! instrument that can be skipped) render 256 batches of twenty silent devices through the engine with
//! the idle skip off and on; only the render loop is timed.
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

use atomik_core::PPQ;
use atomik_model::stress::{
    STRESS_CABLES, STRESS_DEVICES, console_doc, lanes_song_doc, large_song_doc, stress_doc,
    stress_song_doc,
};
use atomik_model::{Command, ConsoleCommand, DeviceKey, Document, RackDoc, SongCommand};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use atomik_testkit::bench::{Recorder, SAMPLES, once_mode, time_once};
use criterion::Criterion;
use std::num::NonZeroUsize;
use std::time::Duration;

/// Bars of each scenario that are bounced (at the document's tempo).
const BARS: i64 = 2;

/// `strips` synth tracks that never play a note, every strip with its filters, equaliser,
/// compressor and parallel compressor on and two sends to a reverb on the first return: a song
/// whose cost is the strips (the synths are quiescent) and the master.
fn silent_strips_doc(strips: usize) -> RackDoc {
    let registry = default_registry();
    let mut doc = Document::new(large_song_doc(&registry, strips, 0, 0));
    let apply = |doc: &mut Document, command: Command| {
        let applied = doc.apply(command, &registry);
        debug_assert!(applied.is_ok(), "{applied:?}");
    };
    let hall = DeviceKey("hall".to_owned());
    apply(
        &mut doc,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.reverb".into(),
            key: Some(hall.clone()),
        },
    );
    apply(
        &mut doc,
        Command::Console(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(hall),
        }),
    );
    let channels: Vec<DeviceKey> = doc
        .doc()
        .console
        .channels
        .iter()
        .map(|c| c.channel.clone())
        .collect();
    for channel in channels {
        for (param, value) in [
            ("hpf.on", 1.0),
            ("comp.on", 1.0),
            ("comp.threshold", -28.0),
            ("eq.on", 1.0),
            ("eq.hf.gain", 3.0),
            ("pcomp.on", 1.0),
            ("send1.on", 1.0),
            ("send2.on", 1.0),
        ] {
            apply(
                &mut doc,
                Command::SetParam {
                    key: channel.clone(),
                    param: param.into(),
                    value,
                    merge: None,
                },
            );
        }
    }
    apply(
        &mut doc,
        Command::Song(SongCommand::SetSongEnd(BARS * 4 * PPQ)),
    );
    doc.doc().clone()
}

fn short(mut doc: RackDoc) -> RackDoc {
    doc.song_mut().end = BARS * 4 * PPQ;
    doc
}

fn scenarios() -> Vec<(&'static str, RackDoc)> {
    let reg = default_registry();
    vec![
        (
            "rack",
            short(stress_doc(&reg, STRESS_DEVICES, STRESS_CABLES)),
        ),
        ("song", short(stress_song_doc(&reg))),
        ("console-64", short(console_doc(&reg, 64))),
        ("silent-strips-64", silent_strips_doc(64)),
        ("lanes-200", short(lanes_song_doc(&reg, 200))),
        ("lanes-2000", short(lanes_song_doc(&reg, 2000))),
        ("large-song", short(large_song_doc(&reg, 100, 500, 2000))),
    ]
}

fn options(workers: u16, block: usize) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap_or(NonZeroUsize::MIN),
        workers,
        loop_count: None,
    }
}

fn bounce(doc: &RackDoc, workers: u16, block: usize) -> usize {
    let reg = default_registry();
    let rendered = render_doc(doc, &reg, &options(workers, block)).expect("scenario renders");
    debug_assert!(rendered.channels.iter().flatten().all(|s| s.is_finite()));
    rendered.channels.first().map_or(0, Vec::len)
}

/// Batches one run of the idle rows renders (a third of a second of audio at 48 kHz, as the
/// device bench).
const IDLE_BATCHES: usize = 256;
/// Effects in each idle row.
const IDLE_DELAYS: usize = 20;
/// The effects and instruments that have an idle row (`idle/<name>-x20/skip-off|on`).
const IDLE_TYPES: [&str; 13] = [
    "atomik.channel",
    "atomik.bus",
    "atomik.stereo-delay",
    "atomik.echo",
    "atomik.reverb",
    "atomik.space-reverb",
    "atomik.modulation",
    "atomik.organ",
    "atomik.sampler",
    "atomik.granular",
    "atomik.fm-synth",
    "atomik.wavetable-synth",
    "atomik.poly-synth",
];

/// The engine of an idle row: twenty effects of `type_id` with nothing connected, so every one
/// reads silence, with the idle skip on or off, rendered once so each is past its first batch.
/// The device bench's `idle` rows call `process` directly and cannot see what the engine skips;
/// this one can. Only the render loop is timed (`render_idle`).
fn idle_engine(type_id: &str, skip: bool) -> (atomik_engine::Engine, atomik_engine::EngineHandle) {
    use atomik_core::{SampleRate, Tempo};
    use atomik_engine::{EngineConfig, Graph, new_engine};
    let config = EngineConfig {
        sample_rate: SampleRate::new(48_000.0).expect("rate"),
        tempo: Tempo::new(120.0).expect("tempo"),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    };
    let registry = default_registry();
    let mut graph = Graph::new(&config);
    for _ in 0..IDLE_DELAYS {
        graph.add_device(registry.create(type_id).expect("registered"));
    }
    let (mut engine, mut handle) = new_engine(config);
    engine.set_idle_skip(skip);
    handle
        .submit_plan(graph.compile().expect("compiles"))
        .expect("queued");
    render_idle(&mut engine);
    (engine, handle)
}

/// One timed run of the idle rows.
fn render_idle(engine: &mut atomik_engine::Engine) -> usize {
    let (mut left, mut right) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..IDLE_BATCHES {
        engine.render(&[], &mut [&mut left, &mut right]);
    }
    IDLE_BATCHES
}

/// Nanoseconds since the first call: the clock the app installs for the per-device CPU readout.
fn bench_clock() -> u64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_nanos() as u64
}

fn main() {
    // `ATOMIK_BENCH_TIMING=1` renders with the per-device timing on, to price its overhead
    // against a run without it.
    if std::env::var_os("ATOMIK_BENCH_TIMING").is_some() {
        atomik_engine::timing::install_clock(bench_clock);
        atomik_engine::timing::set_enabled(true);
        println!("per-device timing on");
    }
    // `ATOMIK_BENCH_GATE=dispatch|inline|load` sets how engines decide when the render workers
    // are used (default: a bounce hands every level over).
    match std::env::var("ATOMIK_BENCH_GATE").as_deref() {
        Ok("dispatch") => {
            atomik_engine::set_default_gate_mode(atomik_engine::GateMode::ForceDispatch)
        }
        Ok("inline") => atomik_engine::set_default_gate_mode(atomik_engine::GateMode::ForceInline),
        Ok("load") => atomik_engine::set_default_gate_mode(atomik_engine::GateMode::Load),
        _ => {}
    }
    // `ATOMIK_BENCH_IDLE=off` bounces with the idle skip off (the scenario rows only; the `idle` rows
    // carry both).
    if std::env::var("ATOMIK_BENCH_IDLE").as_deref() == Ok("off") {
        atomik_engine::set_default_idle_skip(false);
        println!("idle skip off");
    }
    let all = scenarios();
    if once_mode() {
        for (name, doc) in &all {
            let secs = time_once(|| bounce(doc, 0, 256));
            println!("{name}: once {secs:.3} s");
        }
        for type_id in IDLE_TYPES {
            for skip in [false, true] {
                let (mut engine, _handle) = idle_engine(type_id, skip);
                let secs = time_once(|| render_idle(&mut engine));
                println!("idle {type_id} skip {skip}: once {secs:.3} s");
            }
        }
        return;
    }
    let full = std::env::var_os("ATOMIK_BENCH_FULL").is_some();
    let (workers, blocks): (&[u16], &[usize]) = if full {
        (&[0, 1, 2, 4], &[64, 256, 1024])
    } else {
        (&[0, 2], &[256])
    };
    let mut c = Criterion::default()
        .sample_size(SAMPLES)
        .warm_up_time(Duration::from_millis(50))
        .measurement_time(Duration::from_millis(500))
        .configure_from_args();
    for (name, doc) in &all {
        for &w in workers {
            for &b in blocks {
                let id = format!("render/{name}/workers{w}/block{b}");
                let rec = Recorder::new();
                c.bench_function(&id, |bench| {
                    bench.iter_custom(rec.runner(|| bounce(doc, w, b)));
                });
                rec.report(&id);
            }
        }
    }
    for type_id in IDLE_TYPES {
        for skip in [false, true] {
            let id = format!(
                "idle/{}-x{IDLE_DELAYS}/skip-{}",
                type_id.trim_start_matches("atomik."),
                if skip { "on" } else { "off" }
            );
            let (mut engine, _handle) = idle_engine(type_id, skip);
            let rec = Recorder::new();
            c.bench_function(&id, |bench| {
                bench.iter_custom(rec.runner(|| render_idle(&mut engine)));
            });
            rec.report(&id);
        }
    }
    c.final_summary();
}
