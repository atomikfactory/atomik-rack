//! Interim warps: a region with a time map reads its memory asset through the map in real time
//! (right length and timing across a speed change, silence outside the asset and the region),
//! and a song swap that replaces such a track crossfades to the replacement over 5 ms
//! (240 frames at 48 kHz) without allocating on the audio thread (the guard is installed).
#![allow(clippy::needless_range_loop)] // Justification: sample-index loops read clearer than zipped iterators in these checks.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{
    AssetId, HotEdit, SongAsset, SongAudioRegion, SongAudioTrack, TimedEdit, compile_song,
};
use common::audio::*;
use std::sync::Arc;

const SWAP: u64 = 4_096;
const FADE: usize = 240;

fn interim(start: u64, frames: u64, clip_pos: u64, map: &[(f64, f64)]) -> SongAudioTrack {
    let r = SongAudioRegion {
        warp: Some(0),
        clip_pos,
        clip_frames: clip_pos + frames,
        ..SongAudioRegion::plain(AssetId(0), start, 0, frames)
    };
    SongAudioTrack {
        warp_maps: vec![Arc::from(map)],
        ..audio_track_of(1, vec![r])
    }
}

fn run(track: SongAudioTrack, assets: Vec<SongAsset>, frames: usize) -> Vec<Vec<f32>> {
    Bounce::new(audio_input(vec![track], assets), frames).run()
}

#[test]
fn the_interim_read_follows_the_time_map_across_a_speed_change() {
    // Clip frames 0..2000 read the asset at normal speed from frame 1000, 2000..4000 at double
    // speed: a tempo ramp's two halves. The asset is a straight line, which Hermite reproduces.
    let map = [(0.0, 1_000.0), (2_000.0, 3_000.0), (4_000.0, 7_000.0)];
    let out = run(interim(500, 4_000, 0, &map), vec![asset(2, 12_000)], 6_000);
    let source = |i: usize| -> f64 {
        if i < 2_000 {
            1_000.0 + i as f64
        } else {
            3_000.0 + 2.0 * (i - 2_000) as f64
        }
    };
    for n in 0..6_000 {
        let want = if (500..4_500).contains(&n) {
            ((source(n - 500) + 1.0) * 4.0e-5) as f32
        } else {
            0.0
        };
        let got = out[0][n];
        assert!((got - want).abs() < 2.0e-6, "frame {n}: {got} vs {want}");
        assert!(
            (out[1][n] + want).abs() < 2.0e-6,
            "right channel, frame {n}"
        );
    }
}

#[test]
fn a_cut_region_reads_from_its_place_in_the_clip_and_the_asset_end_is_silent() {
    // The region is the tail of a clip: its first frame is clip frame 1000.
    let map = [(0.0, 0.0), (3_000.0, 3_000.0)];
    let out = run(
        interim(256, 1_000, 1_000, &map),
        vec![asset(1, 1_500)],
        2_000,
    );
    for n in 256..1_256 {
        let s = 1_000 + (n - 256);
        let want = if s < 1_500 { value(0, s) } else { 0.0 };
        assert!((out[0][n] - want).abs() < 2.0e-6, "frame {n}");
        assert_eq!(out[0][n], out[1][n], "mono plays on both channels");
    }
    assert!(out[0][..256].iter().all(|s| *s == 0.0));
    assert!(out[0][1_256..].iter().all(|s| *s == 0.0));
}

#[test]
fn a_bad_time_map_is_refused() {
    for map in [
        vec![(0.0, 0.0)],
        vec![(0.0, 0.0), (0.0, 5.0)],
        vec![(0.0, 0.0), (f64::NAN, 5.0)],
    ] {
        let input = audio_input(vec![interim(0, 100, 0, &map)], vec![asset(1, 1_000)]);
        assert!(compile_song(&input).is_err(), "{map:?}");
    }
}

fn half_of(a: &SongAsset) -> SongAsset {
    let SongAsset::Memory { data, .. } = a else {
        unreachable!()
    };
    let scaled: Vec<Vec<f32>> = data
        .iter()
        .map(|c| c.iter().map(|v| v * 0.5).collect())
        .collect();
    SongAsset::memory(scaled.into())
}

/// The interim song (asset 0 read through an identity map), the finished song (plain region of
/// the half-scale asset 1), and a bounce that swaps from the first to the second at `SWAP`.
type Planes = Vec<Vec<f32>>;

fn swap_runs() -> (Planes, Planes, Planes) {
    const N: usize = 8_192;
    let raw = asset(2, 10_000);
    let assets = vec![raw.clone(), half_of(&raw)];
    let map = [(0.0, 0.0), (9_000.0, 9_000.0)];
    let interim_track = interim(0, 9_000, 0, &map);
    let finished = SongAudioRegion::plain(AssetId(1), 0, 0, 9_000);
    let finished_track = audio_track_of(1, vec![finished]);
    let before = audio_input(vec![interim_track.clone()], assets.clone());
    let after = audio_input(vec![finished_track], assets);
    let a = Bounce::new(before.clone(), N).run();
    let b = Bounce::new(after.clone(), N).run();
    let mut swap = Bounce::new(before, N);
    swap.edits.push(TimedEdit {
        frame: SWAP,
        edit: HotEdit::Song(compile_song(&after).unwrap()),
    });
    (a, b, swap.run())
}

#[test]
fn the_finished_buffer_replaces_the_interim_read_with_a_five_millisecond_crossfade() {
    let (before, after, swapped) = swap_runs();
    let s = SWAP as usize;
    for c in 0..2 {
        for n in 0..8_192 {
            let want = if n < s {
                before[c][n]
            } else if n < s + FADE {
                let t = (n - s) as f32 / FADE as f32;
                before[c][n] * (1.0 - t) + after[c][n] * t
            } else {
                after[c][n]
            };
            let got = swapped[c][n];
            assert!((got - want).abs() < 1.0e-6, "channel {c}, frame {n}");
        }
    }
    // The two differ at the swap, so the fade is what keeps the step small.
    let jump = (before[0][s] - after[0][s]).abs();
    assert!(jump > 1.0e-2, "the signals differ ({jump})");
    let step = (s..s + FADE)
        .map(|n| (swapped[0][n + 1] - swapped[0][n]).abs())
        .fold(0.0f32, f32::max);
    assert!(step < jump / 20.0, "no click: step {step}, jump {jump}");
}

#[test]
fn a_swap_between_plain_songs_does_not_crossfade() {
    let assets = vec![asset(2, 10_000), asset(2, 10_000)];
    let one = audio_input(
        vec![audio_track_of(1, vec![region(0, 0, 9_000)])],
        assets.clone(),
    );
    let two = audio_input(
        vec![audio_track_of(
            1,
            vec![SongAudioRegion::plain(AssetId(1), 0, 3_000, 9_000)],
        )],
        assets,
    );
    let mut swap = Bounce::new(one, 6_000);
    swap.edits.push(TimedEdit {
        frame: SWAP,
        edit: HotEdit::Song(compile_song(&two).unwrap()),
    });
    let out = swap.run();
    let n = SWAP as usize;
    assert_eq!(out[0][n].to_bits(), value(0, 3_000 + n).to_bits());
    assert_eq!(out[0][n - 1].to_bits(), value(0, n - 1).to_bits());
}

/// Hand check (`cargo test -p atomik-engine --release --test audio_interim -- --ignored
/// --nocapture`): the cost of 16 stereo tracks (the test rig has 32 outputs) each playing an interim warp at 1.3x speed.
#[test]
#[ignore = "timing only"]
fn sixteen_interim_warps_cost() {
    const SECONDS: usize = 10;
    let frames = 48_000 * SECONDS;
    let map = [(0.0, 0.0), (frames as f64, frames as f64 * 1.3)];
    let tracks: Vec<SongAudioTrack> = (1..=16u32)
        .map(|id| SongAudioTrack {
            device: atomik_engine::DeviceInstanceId(id),
            ..interim(0, frames as u64, 0, &map)
        })
        .collect();
    let input = audio_input(tracks, vec![asset(2, frames * 2)]);
    #[allow(clippy::disallowed_methods)] // Justification: a timing hand check, ignored by default.
    let started = std::time::Instant::now();
    let out = Bounce::new(input, frames).run();
    let took = started.elapsed();
    println!(
        "16 interim warps, {SECONDS} s: {took:?} ({:.1}% of real time)",
        100.0 * took.as_secs_f64() / SECONDS as f64
    );
    assert_eq!(out.len(), 32);
}
