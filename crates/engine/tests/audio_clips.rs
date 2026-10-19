//! Audio-track playback is sample-accurate for clip start, offset, trim,
//! gain and fades against analytic expectations; loop wraps and locates give the exact samples
//! (no stale audio); mono plays on both channels; regions are cut at the asset end.
#![allow(clippy::needless_range_loop)] // Justification: sample-index loops read clearer than zipped iterators in these checks.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::Ticks;
use atomik_engine::{HotEdit, SongAudioRegion, SongFadeCurve, TransportCommand};
use common::audio::*;

const TOTAL: usize = 9_000;

fn bounce(regions: Vec<SongAudioRegion>, assets: usize, frames: usize) -> Vec<Vec<f32>> {
    let input = audio_input(
        vec![audio_track_of(1, regions)],
        vec![asset(assets, 20_000)],
    );
    Bounce::new(input, frames).run()
}

fn bits(x: f32) -> u32 {
    x.to_bits()
}

#[test]
fn start_offset_and_trim_are_sample_accurate() {
    let out = bounce(vec![region(1_000, 500, 2_000)], 2, TOTAL);
    assert_eq!(out.len(), 2);
    for n in 0..TOTAL {
        for (c, ch) in out.iter().enumerate() {
            let want = if (1_000..3_000).contains(&n) {
                value(c, 500 + n - 1_000)
            } else {
                0.0
            };
            assert_eq!(bits(ch[n]), bits(want), "channel {c}, frame {n}");
        }
    }
}

#[test]
fn gain_scales_every_sample() {
    let mut r = region(64, 0, 3_000);
    r.gain = 0.37;
    let out = bounce(vec![r], 2, 4_000);
    for n in 64..3_064 {
        for (c, ch) in out.iter().enumerate() {
            assert_eq!(bits(ch[n]), bits(value(c, n - 64) * 0.37), "frame {n}");
        }
    }
    assert!(out[0][3_064..].iter().all(|s| bits(*s) == 0));
}

fn faded(curve: SongFadeCurve) -> SongAudioRegion {
    let mut r = region(500, 100, 2_000);
    r.fade_in = 300;
    r.fade_out = 400;
    r.fade_curve = curve;
    r.gain = 0.8;
    r
}

fn reference_fade(curve: SongFadeCurve, i: usize) -> f64 {
    let curve = |t: f64| match curve {
        SongFadeCurve::Linear => t,
        SongFadeCurve::EqualPower => (t * std::f64::consts::FRAC_PI_2).sin(),
    };
    let (len, fin, fout) = (2_000usize, 300usize, 400usize);
    let mut g = 1.0;
    if i < fin {
        g *= curve(i as f64 / fin as f64);
    }
    if i + fout >= len {
        g *= curve((len - i) as f64 / fout as f64);
    }
    g
}

#[test]
fn linear_and_equal_power_fades_match_the_analytic_curves() {
    for curve in [SongFadeCurve::Linear, SongFadeCurve::EqualPower] {
        let out = bounce(vec![faded(curve)], 2, 3_000);
        for i in 0..2_000 {
            let want = f64::from(value(0, 100 + i)) * 0.8 * reference_fade(curve, i);
            let got = f64::from(out[0][500 + i]);
            assert!(
                (got - want).abs() < 2e-6,
                "{curve:?} frame {i}: {got} vs {want}"
            );
        }
        // The first fade-in frame is silent, the first full-level frame after it is not faded.
        assert_eq!(bits(out[0][500]), bits(0.0));
        assert_eq!(bits(out[0][800]), bits(value(0, 400) * 0.8));
        // Everything outside the region is silent.
        assert!(out[0][..500].iter().all(|s| bits(*s) == 0));
        assert!(out[0][2_500..].iter().all(|s| bits(*s) == 0));
    }
}

#[test]
fn a_clip_cut_in_two_regions_plays_exactly_like_the_whole_clip() {
    let whole = faded(SongFadeCurve::EqualPower);
    let (a, b) = (
        SongAudioRegion {
            frames: 700,
            ..whole
        },
        SongAudioRegion {
            song_start_frame: 500 + 700,
            source_offset: 100 + 700,
            frames: 1_300,
            clip_pos: 700,
            ..whole
        },
    );
    let one = bounce(vec![whole], 2, 3_000);
    let two = bounce(vec![a, b], 2, 3_000);
    assert!(
        one[0][800..2_000].iter().any(|s| s.abs() > 1e-3),
        "the clip is audible"
    );
    for c in 0..2 {
        let (x, y): (Vec<u32>, Vec<u32>) = (
            one[c].iter().map(|s| s.to_bits()).collect(),
            two[c].iter().map(|s| s.to_bits()).collect(),
        );
        assert_eq!(x, y, "channel {c}");
    }
}

#[test]
fn the_region_is_cut_at_the_asset_end() {
    let input = audio_input(
        vec![audio_track_of(1, vec![region(100, 1_000, 5_000)])],
        vec![asset(2, 1_600)],
    );
    let out = Bounce::new(input, 3_000).run();
    for n in 0..3_000 {
        let want = if (100..700).contains(&n) {
            value(0, 1_000 + n - 100)
        } else {
            0.0
        };
        assert_eq!(bits(out[0][n]), bits(want), "frame {n}");
    }
    // A region starting past the asset end plays nothing and does not panic.
    let input = audio_input(
        vec![audio_track_of(1, vec![region(0, 5_000, 100)])],
        vec![asset(2, 100)],
    );
    let out = Bounce::new(input, 500).run();
    assert!(out.iter().flatten().all(|s| bits(*s) == 0));
}

#[test]
fn mono_assets_play_on_both_channels() {
    let out = bounce(vec![region(0, 10, 1_500)], 1, 2_000);
    for n in 0..1_500 {
        assert_eq!(bits(out[0][n]), bits(value(0, 10 + n)));
        assert_eq!(bits(out[1][n]), bits(value(0, 10 + n)), "frame {n}");
    }
}

#[test]
fn more_than_two_channels_play_the_first_two() {
    let out = bounce(vec![region(0, 0, 500)], 4, 600);
    for n in 0..500 {
        assert_eq!(bits(out[0][n]), bits(value(0, n)));
        assert_eq!(bits(out[1][n]), bits(value(1, n)));
    }
}

fn playing_song_frame(n: usize, loop_frames: Option<(usize, usize)>) -> usize {
    match loop_frames {
        Some((ls, le)) if n >= le => ls + (n - le) % (le - ls),
        _ => n,
    }
}

#[test]
fn loop_wraps_give_the_exact_samples_for_any_block_size() {
    // Two regions across the loop [2000, 5000): the wrap lands in the first one.
    let regions = vec![region(1_000, 0, 3_500), region(4_500, 9_000, 6_000)];
    let expect = |n: usize| {
        let s = playing_song_frame(n, Some((2_000, 5_000)));
        let c = 0;
        if (1_000..4_500).contains(&s) {
            value(c, s - 1_000)
        } else if (4_500..10_500).contains(&s) {
            value(c, 9_000 + s - 4_500)
        } else {
            0.0
        }
    };
    for block in [1usize, 64, 333, 4_096] {
        let mut b = Bounce::new(
            audio_input(
                vec![audio_track_of(1, regions.clone())],
                vec![asset(2, 20_000)],
            ),
            12_000,
        );
        b.block = block;
        b.loop_frames = Some((2_000, 5_000));
        let out = b.run();
        for n in 0..12_000 {
            assert_eq!(bits(out[0][n]), bits(expect(n)), "block {block}, frame {n}");
        }
    }
}

#[test]
fn a_locate_jumps_to_the_exact_samples_at_the_batch_boundary() {
    // Locate to song frame 5000 (tick 3200) at timeline frame 4000: applied at the first batch
    // boundary at or after it, frame 4032 with 64-frame batches.
    let mut b = Bounce::new(
        audio_input(
            vec![audio_track_of(1, vec![region(0, 0, 12_000)])],
            vec![asset(2, 20_000)],
        ),
        8_000,
    );
    b.edits.push(edit(
        4_000,
        HotEdit::Transport(TransportCommand::Locate(Ticks(tick_of(5_000)))),
    ));
    let out = b.run();
    for n in 0..8_000 {
        let s = if n < 4_032 { n } else { 5_000 + n - 4_032 };
        assert_eq!(bits(out[0][n]), bits(value(0, s)), "frame {n}");
    }
}

#[test]
fn stop_silences_the_track_at_the_boundary_and_play_resumes_in_place() {
    let mut b = Bounce::new(
        audio_input(
            vec![audio_track_of(1, vec![region(0, 0, 12_000)])],
            vec![asset(2, 20_000)],
        ),
        6_000,
    );
    b.edits
        .push(edit(1_024, HotEdit::Transport(TransportCommand::Stop)));
    b.edits
        .push(edit(3_072, HotEdit::Transport(TransportCommand::Play)));
    let out = b.run();
    for n in 0..6_000 {
        let want = if n < 1_024 {
            value(0, n)
        } else if n < 3_072 {
            0.0
        } else {
            value(0, 1_024 + n - 3_072)
        };
        assert_eq!(bits(out[0][n]), bits(want), "frame {n}");
    }
}

#[test]
fn two_audio_tracks_play_independently() {
    let mut r2 = region(200, 0, 1_000);
    r2.gain = 0.5;
    let input = audio_input(
        vec![
            audio_track_of(1, vec![region(0, 0, 500)]),
            audio_track_of(2, vec![r2]),
        ],
        vec![asset(2, 5_000)],
    );
    let out = Bounce::new(input, 1_500).run();
    assert_eq!(out.len(), 4);
    for n in 0..1_500 {
        let a = if n < 500 { value(0, n) } else { 0.0 };
        let b = if (200..1_200).contains(&n) {
            value(0, n - 200) * 0.5
        } else {
            0.0
        };
        assert_eq!(bits(out[0][n]), bits(a));
        assert_eq!(bits(out[2][n]), bits(b), "frame {n}");
    }
}
