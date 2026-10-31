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
//! with 4 FX returns), `console-64` (64 strips, gate and compressor on), `lanes-200` and
//! `lanes-2000` (automation lanes), `large-song` (100 tracks, 500 clips, 2,000 notes).
//! Every scenario bounces `BARS` bars plus the release tail, so a run is a few seconds of audio.
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

use atomik_core::PPQ;
use atomik_model::RackDoc;
use atomik_model::stress::{
    STRESS_CABLES, STRESS_DEVICES, console_doc, lanes_song_doc, large_song_doc, stress_doc,
    stress_song_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use atomik_testkit::bench::{Recorder, once_mode, time_once};
use criterion::Criterion;
use std::num::NonZeroUsize;
use std::time::Duration;

/// Bars of each scenario that are bounced (at the document's tempo).
const BARS: i64 = 2;

fn short(mut doc: RackDoc) -> RackDoc {
    doc.song.end = BARS * 4 * PPQ;
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

fn main() {
    let all = scenarios();
    if once_mode() {
        for (name, doc) in &all {
            let secs = time_once(|| bounce(doc, 0, 256));
            println!("{name}: once {secs:.3} s");
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
        .sample_size(21)
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
    c.final_summary();
}
