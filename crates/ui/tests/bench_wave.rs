//! Benchmark-style test: `paint_channel` over a 1920-column clip with a tempo-mapped source.
//! Run: `cargo test -p atomik-ui --release --test bench_wave -- --ignored --nocapture`.
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

use atomik_audio_file::{AudioData, PeakPyramid};
use atomik_ui::DisplayList;
use atomik_ui::wave_paint::{WaveChannel, WaveDetail, paint_channel};
use std::time::Instant;
use vello::peniko::Color;

#[test]
#[ignore = "benchmark"]
fn paint_channel_step_one() {
    let ch: Vec<f32> = (0..4_800_000)
        .map(|i| ((i % 1013) as f32 - 506.0) / 506.0)
        .collect();
    let pyramid = PeakPyramid::build(&AudioData::new(48_000, vec![ch]).unwrap()).unwrap();
    let fpp = 2500.0;
    let detail = WaveDetail::pick(&pyramid, None, fpp);
    let w = WaveChannel {
        detail,
        ch: 0,
        cy: 40.0,
        half: 30.0,
        gain: 1.0,
        color: Color::WHITE,
        x_start: 0.0,
        columns: 1920,
        line_x: (0.0, 1920.0),
        limit: 4_800_000,
        frames_per_pixel: fpp,
    };
    // A source that costs like a tempo map: a few transcendental calls per evaluation.
    let source = |x: f64| x * fpp + (x * 0.001).sin() * 3.0 + (x * 0.002).cos();
    let frame_x = |f: f64| f / fpp;
    let mut dl = DisplayList::new();
    paint_channel(&mut dl, &w, &source, &frame_x);
    let mut times = Vec::new();
    for _ in 0..21 {
        let t = Instant::now();
        for _ in 0..20 {
            let mut dl = DisplayList::new();
            paint_channel(&mut dl, &w, &source, &frame_x);
        }
        times.push(t.elapsed() / 20);
    }
    times.sort();
    eprintln!("paint_channel 1920 columns: median {:?}", times[10]);
}
