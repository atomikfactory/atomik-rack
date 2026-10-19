//! Benchmarks (ignored; run with `cargo test --release -p atomik-engine --test song_bench --
//! --ignored --nocapture`): song compile time for the size (8 tracks, 64 bars, with
//! automation; budget < 5 ms) and the RT player's cost per batch.
#![allow(clippy::disallowed_methods)] // Justification: wall-clock timing of a benchmark only.

mod common;

use atomik_core::{PPQ, Ticks};
use atomik_engine::{
    Graph, SongAutomation, SongCurve, SongInput, SongPoint, SongTrack, TransportCommand,
    compile_song,
};
use atomik_testkit::fakes::CallLog;
use common::song::{snote, tempo};
use common::*;
use std::time::{Duration, Instant};

const BARS: i64 = 64;

/// 8 tracks x 64 bars: 16th notes (8192 notes), two automation lanes per track with a point
/// every beat, and a tempo change every 8 bars.
fn e3_2_song(devices: &[atomik_engine::DeviceInstanceId]) -> SongInput {
    let tracks = devices
        .iter()
        .enumerate()
        .map(|(t, &device)| SongTrack {
            device,
            notes_socket: CallLog::NOTES_IN,
            notes: (0..BARS * 16)
                .map(|k| snote(k * PPQ / 4, PPQ / 8, 36 + ((k * 7 + t as i64) % 48) as u8))
                .collect(),
            automation: (0..2u32)
                .map(|p| SongAutomation {
                    param: atomik_core::ParamId(p),
                    points: (0..BARS * 4)
                        .map(|b| SongPoint {
                            tick: Ticks(b * PPQ),
                            value: ((b % 8) as f32) / 8.0,
                            curve: if b % 3 == 0 {
                                SongCurve::Step
                            } else {
                                SongCurve::Linear
                            },
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    let map: Vec<(i64, f64)> = (0..BARS / 8)
        .map(|k| (k * 8 * 4 * PPQ, 100.0 + 5.0 * k as f64))
        .collect();
    SongInput {
        tracks,
        tempo: tempo(&map),
        beats_per_bar: 4,
        audio: Vec::new(),
        assets: Vec::new(),
    }
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort_unstable();
    v[v.len() / 2]
}

#[test]
#[ignore = "benchmark"]
fn bench_compile_e3_2_song() {
    let ids: Vec<_> = (1..=8).map(atomik_engine::DeviceInstanceId).collect();
    let input = e3_2_song(&ids);
    let times: Vec<Duration> = (0..21)
        .map(|_| {
            let t = Instant::now();
            let snap = compile_song(&input).unwrap();
            let d = t.elapsed();
            drop(std::hint::black_box(snap));
            d
        })
        .collect();
    let m = median(times);
    eprintln!("compile_song (8 tracks, 64 bars, 8192 notes, 16 lanes): median {m:?}");
    assert!(m < Duration::from_millis(5), "R17 budget: {m:?}");
}

#[test]
#[ignore = "benchmark"]
fn bench_player_cost_per_batch() {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let ids: Vec<_> = (0..8).map(|_| g.add_device(CallLog::boxed().0)).collect();
    let (mut e, mut h) = engine_with(c, 0, None);
    h.submit_plan(g.compile().unwrap()).unwrap();
    h.submit_song(compile_song(&e3_2_song(&ids)).unwrap())
        .unwrap();
    h.transport(TransportCommand::Metronome {
        enabled: true,
        gain: 0.5,
    })
    .unwrap();
    let batches = 20_000usize;
    let mut out = vec![vec![0.0f32; 4096]; 2];
    let mut time = |e: &mut atomik_engine::Engine| {
        let t = Instant::now();
        for _ in 0..batches * 64 / 4096 {
            let mut refs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
            e.render(&[], &mut refs);
        }
        t.elapsed()
    };
    let stopped = time(&mut e);
    h.transport(TransportCommand::Play).unwrap();
    let playing = time(&mut e);
    let per = |d: Duration| d.as_nanos() as f64 / batches as f64;
    eprintln!(
        "per batch: stopped {:.0} ns, playing {:.0} ns (8 tracks, 16 lanes, metronome); player cost ~{:.0} ns",
        per(stopped),
        per(playing),
        per(playing) - per(stopped)
    );
}
