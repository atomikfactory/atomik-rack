//! Interim warps that keep their pitch: a Beats or Tonal region, or one with a transposition,
//! plays through the real-time stretcher while its finished buffer is rendered, so it sounds
//! at the transposed pitch whatever the stretch; a Repitch region without transposition still
//! reads through four-point Hermite, bit for bit; and the 5 ms handoff to the finished buffer
//! meets output that is already at the right pitch and in time. The audio thread never
//! allocates (the guard is installed and the bounce runs inside it).
#![allow(clippy::needless_range_loop)] // Justification: sample-index loops read clearer than zipped iterators in these checks.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_dsp::stretch::{StretchMode, StretchParams, TimeMap, pitch_ratio_for_cents, stretch};
use atomik_engine::{
    AssetId, HotEdit, SongAsset, SongAudioRegion, SongAudioTrack, SongWarpMode, TimedEdit,
    compile_song,
};
use common::audio::*;
use std::sync::Arc;

const SR: f64 = 48_000.0;

/// A stereo asset: a sine at `hz`, quiet (0.1) before frame `loud_from` and loud (0.8) after.
fn tone(hz: f64, frames: usize, loud_from: usize) -> Vec<Vec<f32>> {
    let l: Vec<f32> = (0..frames)
        .map(|i| {
            let amp = if i < loud_from { 0.1 } else { 0.8 };
            (amp * (std::f64::consts::TAU * hz * i as f64 / SR).sin()) as f32
        })
        .collect();
    vec![l.clone(), l]
}

fn interim(
    frames: u64,
    map: &[(f64, f64)],
    warp_mode: SongWarpMode,
    warp_cents: i32,
) -> SongAudioTrack {
    let r = SongAudioRegion {
        warp: Some(0),
        warp_mode,
        warp_cents,
        ..SongAudioRegion::plain(AssetId(0), 0, 0, frames)
    };
    SongAudioTrack {
        warp_maps: vec![Arc::from(map)],
        ..audio_track_of(1, vec![r])
    }
}

/// Frequency of a steady tone from its rising zero crossings (linear interpolation).
fn pitch(x: &[f32]) -> f64 {
    let mut first = None;
    let mut last = 0.0;
    let mut count = 0usize;
    for i in 1..x.len() {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            let at = (i - 1) as f64 + f64::from(-x[i - 1]) / f64::from(x[i] - x[i - 1]);
            if first.is_none() {
                first = Some(at);
            } else {
                count += 1;
            }
            last = at;
        }
    }
    count as f64 * SR / (last - first.unwrap_or(0.0))
}

fn near(got: f64, want: f64, tol: f64) -> bool {
    (got - want).abs() <= tol * want
}

#[test]
fn a_transposed_tonal_or_beats_interim_region_plays_at_the_transposed_pitch() {
    let frames = 96_000usize;
    let assets = vec![SongAsset::memory(tone(440.0, frames, frames).into())];
    // Stretched to 1.25 times the length and 5 semitones up.
    let map = [(0.0, 0.0), (80_000.0, 64_000.0)];
    let want = 440.0 * pitch_ratio_for_cents(500);
    for mode in [SongWarpMode::Tonal, SongWarpMode::Beats] {
        let track = interim(80_000, &map, mode, 500);
        let out = Bounce::new(audio_input(vec![track], assets.clone()), 80_000).run();
        let hz = pitch(&out[0][10_000..70_000]);
        assert!(near(hz, want, 0.01), "{mode:?}: {hz} Hz, want {want}");
        assert!(near(pitch(&out[1][10_000..70_000]), want, 0.01));
    }
    // Repitch without a transposition still resamples: the pitch follows the speed.
    let track = interim(80_000, &map, SongWarpMode::Repitch, 0);
    let out = Bounce::new(audio_input(vec![track], assets), 80_000).run();
    let hz = pitch(&out[0][10_000..70_000]);
    assert!(near(hz, 440.0 * 0.8, 0.01), "Repitch: {hz} Hz");
}

/// The engine's four-point Hermite read of the interim warp, written out.
fn hermite(plane: &[f32], s: f64) -> f32 {
    let i = s.floor();
    let frames = plane.len() as f64;
    if i < -2.0 || i >= frames + 1.0 {
        return 0.0;
    }
    let at = |j: f64| {
        if j >= 0.0 && j < frames {
            plane[j as usize]
        } else {
            0.0
        }
    };
    let (y0, y1, y2, y3) = (at(i - 1.0), at(i), at(i + 1.0), at(i + 2.0));
    let t = (s - i) as f32;
    if t == 0.0 {
        return y1;
    }
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

#[test]
fn a_repitch_interim_region_without_transposition_is_the_hermite_read_bit_for_bit() {
    let map = [(0.0, 100.25), (3_000.0, 3_400.75), (6_000.0, 5_100.5)];
    let a = planes(2, 10_000);
    let track = interim(6_000, &map, SongWarpMode::Repitch, 0);
    let out = Bounce::new(
        audio_input(vec![track], vec![SongAsset::memory(Arc::clone(&a))]),
        6_400,
    )
    .run();
    let m = TimeMap::new(&map).unwrap();
    for n in 0..6_400 {
        let (wl, wr) = if n < 6_000 {
            let s = m.src_at(n as f64);
            (hermite(&a[0], s), hermite(&a[1], s))
        } else {
            (0.0, 0.0)
        };
        assert_eq!(out[0][n].to_bits(), wl.to_bits(), "left, frame {n}");
        assert_eq!(out[1][n].to_bits(), wr.to_bits(), "right, frame {n}");
    }
}

#[test]
fn a_mode_or_transposition_needs_an_interim_warp_and_a_sane_range() {
    let assets = vec![asset(2, 1_000)];
    let plain = SongAudioRegion {
        warp_cents: 300,
        ..region(0, 0, 100)
    };
    let input = audio_input(vec![audio_track_of(1, vec![plain])], assets.clone());
    assert!(
        compile_song(&input).is_err(),
        "a transposition without a warp"
    );
    let map = [(0.0, 0.0), (100.0, 100.0)];
    let far = interim(100, &map, SongWarpMode::Tonal, 2_500);
    assert!(compile_song(&audio_input(vec![far], assets.clone())).is_err());
    let ok = interim(100, &map, SongWarpMode::Tonal, -2_400);
    assert!(compile_song(&audio_input(vec![ok], assets)).is_ok());
}

/// The first frame whose level passes `level`.
fn onset(x: &[f32], level: f32) -> usize {
    x.iter().position(|v| v.abs() > level).unwrap()
}

const SWAP: u64 = 20_480; // a batch boundary
const FADE: usize = 240;

#[test]
fn the_stretched_preview_meets_the_finished_buffer_in_time_and_hands_off_without_a_click() {
    let frames = 96_000usize;
    let source = tone(440.0, frames, 30_000);
    let map = [(0.0, 0.0), (75_000.0, 60_000.0)];
    let len = 75_000usize;
    let ratio = pitch_ratio_for_cents(500);
    // The worker's buffer: the offline Beats stretch of the same map and transposition.
    let refs: Vec<&[f32]> = source.iter().map(Vec::as_slice).collect();
    let params = StretchParams {
        mode: StretchMode::Beats,
        pitch_ratio: ratio,
        sample_rate: SR as u32,
    };
    let buffer = stretch(&refs, &TimeMap::new(&map).unwrap(), len, &params, &|| false).unwrap();
    let assets = vec![
        SongAsset::memory(source.into()),
        SongAsset::memory(buffer.into()),
    ];
    let before = audio_input(
        vec![interim(len as u64, &map, SongWarpMode::Beats, 500)],
        assets.clone(),
    );
    let finished = SongAudioRegion::plain(AssetId(1), 0, 0, len as u64);
    let after = audio_input(vec![audio_track_of(1, vec![finished])], assets);
    let preview = Bounce::new(before.clone(), 80_000).run();
    let done = Bounce::new(after.clone(), 80_000).run();
    // The marked onset (the tone turning loud) sounds at the same place in both, where the map
    // puts it (37 500), within a few milliseconds.
    let (p, d) = (onset(&preview[0], 0.45), onset(&done[0], 0.45));
    let ms = (p as f64 - d as f64).abs() * 1_000.0 / SR;
    assert!(ms <= 8.0, "onsets at {p} and {d} ({ms} ms apart)");
    // The swap: no step in the crossfade is larger than the tone's own steps allow.
    let mut swap = Bounce::new(before, 30_000);
    swap.edits.push(TimedEdit {
        frame: SWAP,
        edit: HotEdit::Song(compile_song(&after).unwrap()),
    });
    let out = swap.run();
    let s = SWAP as usize;
    let tone_step = (s - 2_000..s)
        .map(|n| (out[0][n + 1] - out[0][n]).abs())
        .fold(0.0f32, f32::max);
    let fade_step = (s - 1..s + FADE + 1)
        .map(|n| (out[0][n + 1] - out[0][n]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        fade_step < tone_step * 1.5,
        "no click: step {fade_step} in the handoff, {tone_step} in the tone"
    );
    for n in s + FADE..30_000 {
        assert_eq!(out[0][n].to_bits(), done[0][n].to_bits(), "frame {n}");
    }
    assert!(near(pitch(&out[0][s + FADE..30_000]), 440.0 * ratio, 0.01));
}

#[test]
fn a_locate_restarts_the_stretcher_cleanly() {
    // Looping a stretched, transposed region: every wrap restarts the stretcher, and the output
    // stays at the transposed pitch with no burst at the wrap.
    let frames = 96_000usize;
    let assets = vec![SongAsset::memory(tone(440.0, frames, frames).into())];
    let map = [(0.0, 0.0), (80_000.0, 64_000.0)];
    let mut b = Bounce::new(
        audio_input(
            vec![interim(80_000, &map, SongWarpMode::Tonal, -700)],
            assets,
        ),
        40_000,
    );
    b.loop_frames = Some((5_000, 15_000));
    let out = b.run();
    let peak = out[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak < 0.13, "no burst ({peak})");
    let want = 440.0 * pitch_ratio_for_cents(-700);
    for (a, z) in [(16_000, 24_000), (26_000, 34_000)] {
        let hz = pitch(&out[0][a..z]);
        assert!(near(hz, want, 0.01), "{hz} Hz between {a} and {z}");
    }
}

/// Hand check (`cargo test -p atomik-engine --test audio_interim_pitch -- --ignored
/// --nocapture`): the cost of 16 stereo tracks (the test rig has 32 outputs) each playing a
/// transposed Tonal interim warp at 1.3x speed through the real-time stretcher, next to
/// `audio_interim`'s plain read of the same load (perf/BASELINE.md quotes both).
#[test]
#[ignore = "timing only"]
fn sixteen_pitch_keeping_interim_warps_cost() {
    const SECONDS: usize = 10;
    let frames = 48_000 * SECONDS;
    let map = [(0.0, 0.0), (frames as f64, frames as f64 * 1.3)];
    let tracks: Vec<SongAudioTrack> = (1..=16u32)
        .map(|id| SongAudioTrack {
            device: atomik_engine::DeviceInstanceId(id),
            ..interim(frames as u64, &map, SongWarpMode::Tonal, 300)
        })
        .collect();
    let assets = vec![SongAsset::memory(tone(440.0, frames * 2, 0).into())];
    let input = audio_input(tracks, assets);
    #[allow(clippy::disallowed_methods)] // Justification: a timing hand check, ignored by default.
    let started = std::time::Instant::now();
    let out = Bounce::new(input, frames).run();
    let took = started.elapsed();
    println!(
        "16 pitch-keeping interim warps, {SECONDS} s: {took:?} ({:.1}% of real time)",
        100.0 * took.as_secs_f64() / SECONDS as f64
    );
    assert_eq!(out.len(), 32);
}
