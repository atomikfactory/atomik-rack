//! Callback timing over a real project: the engine is built the way a bounce builds it, then driven
//! from a thread on an absolute-deadline schedule like an audio callback (the period is the buffer
//! over the sample rate, each deadline is the start plus a whole number of periods, so a slow
//! callback does not push the next one back). The time of each `Engine::render` call goes into
//! the histogram of `atomik_audio_io` (the one `atomik-play --stats` reads); the table has one
//! row per project, buffer size and worker count: p50, p99 and max in microseconds, the
//! callbacks that took longer than their period (misses) and the share of the period that the
//! p99 uses.
//!
//! No audio device is opened and nothing is promoted: the thread is an ordinary one, and between
//! two callbacks it spins (a sleep is too coarse for a 64-frame period), so its caches and clock
//! are warmer than a driver's thread. The numbers are a comparison between builds on one machine,
//! not a promise about another. Run in release:
//!
//! ```text
//! cargo test --release -p atomik-render --test callback_timing -- --ignored --nocapture
//! ```
//!
//! `ATOMIK_TIMING_SECONDS` sets the audio per row (default 30), `ATOMIK_TIMING_OUT` a file the
//! table is also written to. The plain test runs a fraction of a second of one row to keep the
//! harness itself honest.
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock pacing and timing is the point of this harness"
)]
atomik_testkit::install_alloc_guard!();

use atomik_audio_io::{CallbackStats, TIME_BUCKETS, time_bucket};
use atomik_core::{PPQ, Ticks};
use atomik_engine::{
    Engine, EngineHandle, EngineOptions, TransportCommand, WorkerConfig, compile_song,
    new_engine_with_options,
};
use atomik_model::RackDoc;
use atomik_model::stress::stress_song_doc;
use atomik_render::build::build_with_devices;
use atomik_render::project::{
    console_options, expanded_ids, player_data_maps, sampler_maps, song_input_live,
};
use atomik_render::{bundle, default_registry};
use atomik_testkit::no_alloc::assert_no_alloc;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SAMPLE_RATE: f64 = 48_000.0;
const BUFFERS: [usize; 4] = [64, 128, 256, 512];
const WORKERS: [u16; 2] = [0, 2];
/// Audio per row when `ATOMIK_TIMING_SECONDS` is not set.
const DEFAULT_SECONDS: f64 = 30.0;
/// Callbacks before the timed ones: the workers start, the pages the song touches are in.
const WARM_UP_SECONDS: f64 = 1.0;
/// The factory demo with the most work per second of audio (see `perf/BASELINE.md`).
const HEAVY_DEMO: &str = "fx-demo";

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects")
}

/// A live engine playing `doc` in a loop over its whole length, so any number of callbacks finds
/// the song still sounding. The build is `render_core`'s without plugins and taps.
fn live_engine(doc: &RackDoc, bundle_root: Option<&Path>, workers: u16) -> (Engine, EngineHandle) {
    let registry = default_registry();
    let bundle = bundle_root.and_then(|root| atomik_audio_file::Bundle::open(root).ok());
    let (assets, _) =
        bundle::load_song_assets_counted(doc, bundle.as_ref()).expect("the assets load");
    let warps = atomik_render::warp::render_warps(doc, &assets);
    let expanded = doc.expand(&registry);
    let (patch, patch_ids) = atomik_render::flat::expanded_patch(doc, &expanded);
    let patch_id = |key: String| {
        patch_ids
            .get(&atomik_model::DeviceKey(key.clone()))
            .cloned()
            .unwrap_or(key)
    };
    let mut options = console_options(doc, &registry);
    options.omit = expanded
        .devices
        .iter()
        .filter(|d| d.frozen)
        .map(|d| patch_id(d.key.0.clone()))
        .collect();
    let (maps, _) = sampler_maps(&expanded, &registry, &assets);
    options.samples = maps.into_iter().map(|(k, m)| (patch_id(k), m)).collect();
    options.player_data = player_data_maps(doc, &registry, &expanded)
        .into_iter()
        .map(|(k, d)| (patch_id(k), d))
        .collect();
    let built = build_with_devices(&patch, &registry, &options, Default::default())
        .expect("the project builds");
    let ids = expanded_ids(&expanded);
    let input = song_input_live(doc, &registry, |k| ids.get(k).copied(), &assets, &warps);
    let snapshot = compile_song(&input).expect("the song compiles");
    let (engine, mut handle) = new_engine_with_options(
        built.config,
        EngineOptions {
            workers: WorkerConfig {
                threads: workers,
                on_thread_start: None,
            },
            worker_linger_spins: 0,
            // A live engine: the workers help only while the audio thread needs it.
            realtime: true,
        },
    );
    let end = doc.song.end.max(4 * PPQ);
    handle.submit_plan(built.plan).expect("plan queued");
    handle.submit_song(snapshot).expect("song queued");
    for command in [
        TransportCommand::SetLoop {
            start: Ticks(0),
            end: Ticks(end),
            enabled: true,
        },
        TransportCommand::Play,
    ] {
        handle.transport(command).expect("transport queued");
    }
    (engine, handle)
}

/// Waits for `deadline`, sleeping only while it is far away (a sleep rounds up to the timer tick,
/// which on some systems is longer than a whole callback).
fn wait_for(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let left = deadline - now;
        if left > Duration::from_millis(20) {
            std::thread::sleep(left - Duration::from_millis(20));
        } else {
            std::thread::yield_now();
            std::hint::spin_loop();
        }
    }
}

/// One row of the table.
struct Row {
    project: String,
    buffer: usize,
    workers: u16,
    callbacks: u64,
    stats: CallbackStats,
    misses: u64,
    period_ns: u64,
}

impl Row {
    fn p99_share(&self) -> f64 {
        self.stats.time_percentile_ns(99.0) as f64 / self.period_ns as f64
    }

    fn line(&self) -> String {
        let us = |ns: u64| ns as f64 / 1_000.0;
        format!(
            "| {} | {} | {} | {} | {:.1} | {:.1} | {:.1} | {} | {:.1} % |",
            self.project,
            self.buffer,
            self.workers,
            self.callbacks,
            us(self.stats.time_percentile_ns(50.0)),
            us(self.stats.time_percentile_ns(99.0)),
            us(self.stats.max_ns),
            self.misses,
            self.p99_share() * 100.0
        )
    }
}

const HEADER: &str = "| project | buffer | workers | callbacks | p50 us | p99 us | max us | misses | p99 / period |\n\
                      |---|---|---|---|---|---|---|---|---|";

/// Drives `engine` for `seconds` of audio in callbacks of `buffer` frames on the absolute
/// schedule and records each callback's processing time.
fn run_row(
    project: &str,
    doc: &RackDoc,
    root: Option<&Path>,
    buffer: usize,
    workers: u16,
    seconds: f64,
) -> Row {
    let (mut engine, mut handle) = live_engine(doc, root, workers);
    let channels = engine.output_channels();
    let mut out = vec![vec![0.0f32; buffer]; channels];
    let period = Duration::from_secs_f64(buffer as f64 / SAMPLE_RATE);
    let callbacks = ((seconds * SAMPLE_RATE) / buffer as f64).ceil() as u64;
    let warm_up = ((WARM_UP_SECONDS * SAMPLE_RATE) / buffer as f64).ceil() as u64;
    let mut stats = CallbackStats::default();
    let mut misses = 0;
    let start = Instant::now() + Duration::from_millis(50);
    for k in 0..warm_up + callbacks {
        wait_for(start + period * k as u32);
        let began = Instant::now();
        let mut outs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
        engine.render(&[], &mut outs);
        let took = began.elapsed();
        handle.collect_garbage();
        if k >= warm_up {
            let ns = u64::try_from(took.as_nanos()).unwrap_or(u64::MAX);
            stats.time[time_bucket(ns).min(TIME_BUCKETS - 1)] += 1;
            stats.max_ns = stats.max_ns.max(ns);
            misses += u64::from(took > period);
        }
    }
    assert!(
        out.iter().flatten().all(|s| s.is_finite()),
        "the project rendered a non-finite sample"
    );
    Row {
        project: project.to_string(),
        buffer,
        workers,
        callbacks: stats.callbacks(),
        stats,
        misses,
        period_ns: u64::try_from(period.as_nanos()).unwrap_or(u64::MAX),
    }
}

/// The projects of the table: the heaviest factory demo and the generated stress song the
/// benches use.
fn projects() -> Vec<(String, RackDoc, Option<PathBuf>)> {
    let registry = default_registry();
    let file = examples()
        .join(HEAVY_DEMO)
        .join(format!("{HEAVY_DEMO}.atomik"));
    let text = atomik_model::read_document_text(&file).expect("the demo reads");
    let demo = atomik_model::load_project(&text, &registry).expect("the demo loads");
    let root = bundle::existing_bundle_root(&file);
    vec![
        (HEAVY_DEMO.to_string(), demo, root),
        ("stress-song".to_string(), stress_song_doc(&registry), None),
    ]
}

#[test]
fn the_harness_paces_and_records_every_callback() {
    let registry = default_registry();
    let doc = stress_song_doc(&registry);
    let row = run_row("stress-song", &doc, None, 256, 0, 0.3);
    let expected = (0.3 * SAMPLE_RATE / 256.0).ceil() as u64;
    assert_eq!(
        row.callbacks, expected,
        "every callback is in the histogram"
    );
    assert_eq!(row.period_ns, 5_333_333);
    assert!(row.stats.max_ns > 0, "the engine took some time");
    assert!(row.misses <= row.callbacks);
    assert!(
        row.stats.time_percentile_ns(50.0) <= row.stats.max_ns,
        "a percentile never passes the maximum"
    );
}

#[test]
fn the_live_engine_of_the_harness_renders_without_allocating() {
    // The timed loop is not wrapped (the guard would be part of the numbers); this is the same
    // engine, on the same calls, under the guard.
    let registry = default_registry();
    let (mut engine, mut handle) = live_engine(&stress_song_doc(&registry), None, 0);
    let mut out = vec![vec![0.0f32; 128]; engine.output_channels()];
    for k in 0..300 {
        let mut outs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
        if k < 20 {
            engine.render(&[], &mut outs);
        } else {
            assert_no_alloc(|| engine.render(&[], &mut outs));
        }
        handle.collect_garbage();
    }
}

#[test]
#[ignore = "timing run of minutes; run in release: cargo test --release -p atomik-render --test callback_timing -- --ignored --nocapture"]
fn callback_timing_release() {
    let seconds = std::env::var("ATOMIK_TIMING_SECONDS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| *s > 0.0)
        .unwrap_or(DEFAULT_SECONDS);
    let mut table = format!(
        "callback timing, {seconds} s of audio per row at 48 kHz, realtime engine\n\n{HEADER}\n"
    );
    println!("{table}");
    for (name, doc, root) in projects() {
        for buffer in BUFFERS {
            for workers in WORKERS {
                let row = run_row(&name, &doc, root.as_deref(), buffer, workers, seconds);
                let line = row.line();
                println!("{line}");
                table.push_str(&line);
                table.push('\n');
            }
        }
    }
    if let Ok(path) = std::env::var("ATOMIK_TIMING_OUT") {
        std::fs::write(&path, &table).expect("the table is written");
    }
}
