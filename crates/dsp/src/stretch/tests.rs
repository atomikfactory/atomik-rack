use super::fft::atan2_det;
use super::*;
use crate::test_util::{dominant_hz, noise, rms, sine};

const SR: u32 = 48_000;

fn never() -> bool {
    false
}

fn run(
    source: &[Vec<f32>],
    map: &TimeMap,
    frames: usize,
    mode: StretchMode,
    ratio: f64,
) -> Vec<Vec<f32>> {
    let refs: Vec<&[f32]> = source.iter().map(Vec::as_slice).collect();
    let params = StretchParams {
        mode,
        pitch_ratio: ratio,
        sample_rate: SR,
    };
    stretch(&refs, map, frames, &params, &never).expect("not cancelled")
}

/// The middle part of a buffer, away from the start and end transients of the stretcher.
fn middle(x: &[f32]) -> &[f32] {
    &x[x.len() / 4..x.len() * 3 / 4]
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs()
}

#[test]
fn the_map_maps_both_ways_and_extends_its_ends() {
    let m = TimeMap::new(&[(0.0, 0.0), (100.0, 50.0), (200.0, 250.0)]).unwrap();
    assert!(near(m.src_at(50.0), 25.0, 1e-9));
    assert!(near(m.src_at(150.0), 150.0, 1e-9));
    assert!(near(m.src_at(-10.0), -5.0, 1e-9));
    assert!(near(m.src_at(250.0), 350.0, 1e-9));
    for out in [0.5, 60.0, 100.0, 170.0] {
        assert!((m.out_at(m.src_at(out)) - out).abs() < 1e-9, "{out}");
    }
    assert!(TimeMap::new(&[(0.0, 0.0)]).is_none());
    assert!(TimeMap::new(&[(0.0, 0.0), (0.0, 1.0)]).is_none());
    assert!(TimeMap::new(&[(0.0, 5.0), (1.0, 4.0)]).is_none());
    assert!(TimeMap::new(&[(0.0, f64::NAN), (1.0, 4.0)]).is_none());
    let s = m.scaled_out(2.0);
    assert!(near(s.src_at(100.0), 25.0, 1e-9));
}

#[test]
fn the_arctangent_is_close_to_the_library_one() {
    let mut worst = 0.0f64;
    for i in -200..=200 {
        for j in -200..=200 {
            let (y, x) = (f64::from(i) * 0.37, f64::from(j) * 0.21);
            if x == 0.0 && y == 0.0 {
                continue;
            }
            worst = worst.max((atan2_det(y, x) - y.atan2(x)).abs());
        }
    }
    assert!(worst < 2.0e-5, "worst {worst}");
}

#[test]
fn the_pitch_ratio_of_cents() {
    assert!(near(pitch_ratio_for_cents(1200), 2.0, 1e-9));
    assert!(near(pitch_ratio_for_cents(-1200), 0.5, 1e-9));
    assert!(near(pitch_ratio_for_cents(0), 1.0, 1e-12));
    assert!(near(pitch_ratio_for_cents(2400), 4.0, 1e-9));
}

#[test]
fn stretching_keeps_the_pitch_in_every_mode_that_keeps_it() {
    let src = vec![sine(440.0, f64::from(SR), SR as usize, 0.5)];
    for mode in [StretchMode::Tonal, StretchMode::Beats] {
        // Half speed: twice as long.
        let slow = run(&src, &TimeMap::linear(0.5), 2 * SR as usize, mode, 1.0);
        assert_eq!(slow[0].len(), 2 * SR as usize);
        let hz = dominant_hz(middle(&slow[0]), f64::from(SR));
        assert!(near(hz, 440.0, 0.01), "{mode:?} 0.5x: {hz}");
        assert!(rms(middle(&slow[0])) > 0.3, "{mode:?} level");
        // Double speed: half as long.
        let fast = run(&src, &TimeMap::linear(2.0), SR as usize / 2, mode, 1.0);
        let hz = dominant_hz(middle(&fast[0]), f64::from(SR));
        assert!(near(hz, 440.0, 0.01), "{mode:?} 2x: {hz}");
        assert!(rms(middle(&fast[0])) > 0.3, "{mode:?} level");
    }
}

#[test]
fn repitch_changes_pitch_with_speed() {
    let src = vec![sine(440.0, f64::from(SR), SR as usize, 0.5)];
    let slow = run(
        &src,
        &TimeMap::linear(0.5),
        2 * SR as usize,
        StretchMode::Repitch,
        1.0,
    );
    assert!(near(
        dominant_hz(middle(&slow[0]), f64::from(SR)),
        220.0,
        0.01
    ));
    let fast = run(
        &src,
        &TimeMap::linear(2.0),
        SR as usize / 2,
        StretchMode::Repitch,
        1.0,
    );
    assert!(near(
        dominant_hz(middle(&fast[0]), f64::from(SR)),
        880.0,
        0.01
    ));
}

#[test]
fn a_pitch_shift_moves_the_pitch_and_keeps_the_length() {
    let src = vec![sine(440.0, f64::from(SR), SR as usize, 0.5)];
    let map = TimeMap::linear(1.0);
    for mode in [StretchMode::Tonal, StretchMode::Beats] {
        for (cents, want) in [
            (1200, 880.0),
            (-1200, 220.0),
            (700, 440.0 * 2f64.powf(7.0 / 12.0)),
        ] {
            let ratio = pitch_ratio_for_cents(cents);
            let out = run(&src, &map, SR as usize, mode, ratio);
            assert_eq!(out[0].len(), SR as usize);
            let hz = dominant_hz(middle(&out[0]), f64::from(SR));
            assert!(
                near(hz, want, 0.01),
                "{mode:?} {cents} cents: {hz} vs {want}"
            );
            assert!(out[0].iter().all(|s| s.is_finite()));
        }
    }
}

#[test]
fn ratio_one_is_close_to_the_identity() {
    // A chord with a little noise.
    let n = SR as usize;
    let mut x = sine(220.0, f64::from(SR), n, 0.2);
    for (i, s) in sine(330.0, f64::from(SR), n, 0.2).iter().enumerate() {
        x[i] += s;
    }
    for (i, s) in noise(7, n, 0.02).iter().enumerate() {
        x[i] += s;
    }
    let src = vec![x.clone()];
    let map = TimeMap::linear(1.0);
    let tonal = run(&src, &map, n, StretchMode::Tonal, 1.0);
    let worst = tonal[0]
        .iter()
        .zip(&x)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 0.02, "tonal identity error {worst}");
    let beats = run(&src, &map, n, StretchMode::Beats, 1.0);
    let worst = beats[0]
        .iter()
        .zip(&x)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1.0e-3, "beats identity error {worst}");
    let rep = run(&src, &map, n, StretchMode::Repitch, 1.0);
    let worst = rep[0]
        .iter()
        .zip(&x)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1.0e-6, "repitch identity error {worst}");
}

/// Hits of 8 ms of decaying noise every `period` frames.
fn hits(period: usize, count: usize) -> Vec<f32> {
    let mut x = vec![0.0f32; period * count + period];
    let burst = noise(3, 4000, 0.9);
    for k in 0..count {
        let at = period / 2 + k * period;
        for (i, b) in burst.iter().enumerate() {
            let env = (-(i as f32) / 600.0).exp();
            x[at + i] += b * env;
        }
    }
    x
}

/// First frame at or after `from` whose magnitude is at least `level`.
fn first_over(x: &[f32], from: usize, level: f32) -> usize {
    (from..x.len())
        .find(|&i| x[i].abs() >= level)
        .unwrap_or(usize::MAX)
}

#[test]
fn beats_mode_keeps_the_hits_on_their_stretched_positions() {
    let period = 12_000usize;
    let src = vec![hits(period, 6)];
    for (slope, label) in [(0.5, "half speed"), (2.0, "double speed"), (0.8, "0.8x")] {
        let frames = (src[0].len() as f64 / slope) as usize;
        let out = run(
            &src,
            &TimeMap::linear(slope),
            frames,
            StretchMode::Beats,
            1.0,
        );
        for k in 0..6 {
            let at_src = period / 2 + k * period;
            let want = (at_src as f64 / slope) as usize;
            // The hit's first loud sample sits where the map puts it, to within 3 ms, and the
            // frames just before it are quiet (no smeared pre-echo).
            let got = first_over(&out[0], want.saturating_sub(300), 0.15);
            assert!(
                got.abs_diff(want) < 150,
                "{label}: hit {k} at {got}, wanted {want}"
            );
            let before = &out[0][want.saturating_sub(250)..want.saturating_sub(60)];
            assert!(
                before.iter().all(|s| s.abs() < 0.02),
                "{label}: hit {k} has pre-echo"
            );
        }
    }
}

#[test]
fn a_stereo_source_keeps_its_channels_apart() {
    let n = SR as usize;
    let src = vec![
        sine(300.0, f64::from(SR), n, 0.4),
        sine(900.0, f64::from(SR), n, 0.4),
    ];
    for mode in [StretchMode::Tonal, StretchMode::Beats] {
        let out = run(&src, &TimeMap::linear(0.75), n * 4 / 3, mode, 1.0);
        assert_eq!(out.len(), 2);
        assert!(near(
            dominant_hz(middle(&out[0]), f64::from(SR)),
            300.0,
            0.01
        ));
        assert!(near(
            dominant_hz(middle(&out[1]), f64::from(SR)),
            900.0,
            0.01
        ));
    }
}

#[test]
fn noise_and_odd_maps_stay_finite_and_bounded() {
    let n = 30_000usize;
    let src = vec![noise(1, n, 0.8), noise(2, n, 0.8)];
    // A map that speeds up, stalls, then runs backwards in effect (flat) and past both ends.
    let map = TimeMap::new(&[
        (0.0, -500.0),
        (5000.0, 3000.0),
        (9000.0, 3000.0),
        (30_000.0, 40_000.0),
    ])
    .unwrap();
    for mode in [StretchMode::Repitch, StretchMode::Beats, StretchMode::Tonal] {
        for ratio in [1.0, 0.5, 3.0] {
            let out = run(&src, &map, 30_000, mode, ratio);
            for ch in &out {
                assert_eq!(ch.len(), 30_000);
                assert!(
                    ch.iter().all(|s| s.is_finite() && s.abs() < 4.0),
                    "{mode:?} {ratio}"
                );
            }
        }
    }
    // Silence in, silence out.
    let silent = vec![vec![0.0f32; 20_000]];
    for mode in [StretchMode::Beats, StretchMode::Tonal] {
        let out = run(&silent, &TimeMap::linear(0.5), 40_000, mode, 1.5);
        assert!(out[0].iter().all(|s| *s == 0.0), "{mode:?}");
    }
    // Empty input.
    let empty: Vec<Vec<f32>> = vec![Vec::new()];
    for mode in [StretchMode::Beats, StretchMode::Tonal, StretchMode::Repitch] {
        let out = run(&empty, &TimeMap::linear(1.0), 1000, mode, 1.0);
        assert!(out[0].iter().all(|s| *s == 0.0));
    }
}

#[test]
fn the_result_is_repeatable_and_a_cancel_stops_it() {
    let src = vec![noise(5, 20_000, 0.5)];
    for mode in [StretchMode::Beats, StretchMode::Tonal] {
        let a = run(&src, &TimeMap::linear(0.7), 30_000, mode, 1.3);
        let b = run(&src, &TimeMap::linear(0.7), 30_000, mode, 1.3);
        assert_eq!(a, b, "{mode:?}");
        let refs: Vec<&[f32]> = src.iter().map(Vec::as_slice).collect();
        let params = StretchParams {
            mode,
            pitch_ratio: 1.0,
            sample_rate: SR,
        };
        assert!(stretch(&refs, &TimeMap::linear(0.7), 30_000, &params, &|| true).is_none());
    }
}

#[test]
fn a_tempo_ramp_map_keeps_the_pitch() {
    // The source plays faster and faster: the map's slope grows from 0.5 to 1.5.
    let n = 2 * SR as usize;
    let src = vec![sine(500.0, f64::from(SR), n, 0.5)];
    let knots: Vec<(f64, f64)> = (0..=40)
        .map(|k| {
            let t = f64::from(k) * 2_000.0;
            (t, 0.5 * t + 0.5 * t * t / 80_000.0)
        })
        .collect();
    let map = TimeMap::new(&knots).unwrap();
    let frames = 80_000;
    for mode in [StretchMode::Tonal, StretchMode::Beats] {
        let out = run(&src, &map, frames, mode, 1.0);
        let hz = dominant_hz(middle(&out[0]), f64::from(SR));
        assert!(near(hz, 500.0, 0.02), "{mode:?}: {hz}");
    }
}
