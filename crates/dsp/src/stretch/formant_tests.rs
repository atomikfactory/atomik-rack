//! The formant and stereo-link work of the tonal engine.

use super::fft::Fft;
use super::formant::{Envelope, gains};
use super::*;
use crate::test_util::{spectrum, tone_amplitude};

const SR: u32 = 48_000;
const TAU: f64 = std::f64::consts::TAU;

fn never() -> bool {
    false
}

/// The amplitude a vowel-like envelope gives at `f` Hz: three resonances.
fn vowel(f: f64) -> f64 {
    let bump = |c: f64, w: f64, g: f64| g * (-((f - c) / w).powi(2)).exp();
    0.02 + bump(500.0, 130.0, 1.0) + bump(1500.0, 220.0, 0.6) + bump(2500.0, 300.0, 0.3)
}

/// Harmonics of `f0` shaped by [`vowel`], `seconds` long.
fn voice(f0: f64, seconds: f64) -> Vec<f32> {
    let n = (seconds * f64::from(SR)) as usize;
    let mut out = vec![0.0f32; n];
    let mut h = 1.0;
    while f0 * h < 6000.0 {
        let (f, a) = (f0 * h, vowel(f0 * h) * 0.05);
        let phase = h * 0.37;
        for (i, o) in out.iter_mut().enumerate() {
            *o += (a * (TAU * f * i as f64 / f64::from(SR) + phase).sin()) as f32;
        }
        h += 1.0;
    }
    out
}

fn render(src: &[Vec<f32>], frames: usize, params: &StretchParams) -> Vec<Vec<f32>> {
    let refs: Vec<&[f32]> = src.iter().map(Vec::as_slice).collect();
    stretch(&refs, &TimeMap::linear(1.0), frames, params, &never).expect("not cancelled")
}

/// The smoothed log2 envelope of a 4096-sample piece from the middle of `x`, bin `j` at
/// `j * SR / 4096` Hz.
fn envelope_of(x: &[f32]) -> Vec<f64> {
    let n = 4096;
    let start = x.len() / 2 - n / 2;
    let mut mag = spectrum(&x[start..start + n]);
    mag.push(mag[n / 2 - 1]);
    let (mut zr, mut zi) = (vec![0.0; n], vec![0.0; n]);
    let mut out = vec![0.0; n / 2 + 1];
    Envelope::new(n, SR).log_envelope(&Fft::new(n), &mag, &mut zr, &mut zi, &mut out);
    out
}

/// The frequency of the envelope maximum between `lo` and `hi` Hz.
fn peak_hz(env: &[f64], lo: f64, hi: f64) -> f64 {
    let bin = |hz: f64| (hz * 4096.0 / f64::from(SR)) as usize;
    let k = (bin(lo)..=bin(hi))
        .max_by(|a, b| env[*a].total_cmp(&env[*b]))
        .unwrap();
    k as f64 * f64::from(SR) / 4096.0
}

fn tonal(ratio: f64, keep: bool, formant: f64) -> StretchParams {
    StretchParams {
        keep_formants: keep,
        formant_ratio: formant,
        ..StretchParams::new(StretchMode::Tonal, ratio, SR)
    }
}

#[test]
fn a_transposed_vowel_keeps_its_formant_peaks_while_the_pitch_moves() {
    let src = vec![voice(110.0, 2.0)];
    let before = peak_hz(&envelope_of(&src[0]), 300.0, 900.0);
    assert!((before - 500.0).abs() < 40.0, "source peak {before}");
    let ratio = 1.5;
    let kept = render(&src, 96_000, &tonal(ratio, true, 1.0));
    let after = peak_hz(&envelope_of(&kept[0]), 300.0, 900.0);
    assert!((after - before).abs() / before < 0.08, "kept peak {after}");
    // The pitch did move: the harmonics of 165 Hz are there and the old ones are not.
    let new = tone_amplitude(&kept[0][24_000..72_000], 165.0 * 3.0, f64::from(SR));
    let old = tone_amplitude(&kept[0][24_000..72_000], 110.0 * 5.0, f64::from(SR));
    assert!(new > 8.0 * old, "new {new} old {old}");
    // Without it the envelope rides along with the pitch.
    let moved = render(&src, 96_000, &tonal(ratio, false, 1.0));
    let after = peak_hz(&envelope_of(&moved[0]), 300.0, 900.0);
    assert!(after > before * 1.3, "unkept peak {after}");
}

#[test]
fn a_transposed_down_vowel_keeps_its_second_formant_too() {
    let src = vec![voice(160.0, 2.0)];
    let before = peak_hz(&envelope_of(&src[0]), 1100.0, 2000.0);
    let kept = render(&src, 96_000, &tonal(0.75, true, 1.0));
    let after = peak_hz(&envelope_of(&kept[0]), 1100.0, 2000.0);
    assert!((after - before).abs() / before < 0.1, "{before} -> {after}");
}

#[test]
fn a_transposed_chord_keeps_its_envelope() {
    // Three voices on one vowel, a major chord.
    let a = voice(110.0, 2.0);
    let b = voice(110.0 * 1.2599, 2.0);
    let c = voice(110.0 * 1.4983, 2.0);
    let chord: Vec<f32> = (0..a.len()).map(|i| a[i] + b[i] + c[i]).collect();
    let src = vec![chord];
    let before = peak_hz(&envelope_of(&src[0]), 300.0, 900.0);
    let kept = render(&src, 96_000, &tonal(1.4, true, 1.0));
    let after = peak_hz(&envelope_of(&kept[0]), 300.0, 900.0);
    assert!((after - before).abs() / before < 0.1, "{before} -> {after}");
    let moved = render(&src, 96_000, &tonal(1.4, false, 1.0));
    let after = peak_hz(&envelope_of(&moved[0]), 300.0, 900.0);
    assert!(after > before * 1.2, "unkept {after}");
}

#[test]
fn the_formant_shift_moves_the_envelope_without_moving_the_pitch() {
    let src = vec![voice(110.0, 2.0)];
    let before = peak_hz(&envelope_of(&src[0]), 300.0, 900.0);
    let up = render(&src, 96_000, &tonal(1.0, true, 1.5));
    let after = peak_hz(&envelope_of(&up[0]), 300.0, 1100.0);
    assert!(after > before * 1.3, "shifted peak {after} from {before}");
    // The harmonics stay where they were.
    let same = tone_amplitude(&up[0][24_000..72_000], 110.0 * 8.0, f64::from(SR));
    let between = tone_amplitude(&up[0][24_000..72_000], 110.0 * 8.5, f64::from(SR));
    assert!(same > 4.0 * between, "{same} {between}");
}

#[test]
fn no_shift_means_the_gains_are_one() {
    let l: Vec<f64> = (0..65).map(|j| (j as f64 * 0.3).sin()).collect();
    let mut g = vec![0.0; 65];
    gains(&l, 1.0, &mut g);
    assert!(g.iter().all(|v| (v - 1.0).abs() < 1.0e-9));
    gains(&l, 2.0, &mut g);
    assert!(g.iter().all(|v| (1.0 / 16.0..=16.0).contains(v)));
}

#[test]
fn keeping_formants_is_deterministic_and_leaves_a_plain_render_alone() {
    let src = vec![voice(110.0, 1.0), voice(130.0, 1.0)];
    let p = StretchParams {
        link_stereo: true,
        ..tonal(1.3, true, 1.0)
    };
    assert_eq!(render(&src, 40_000, &p), render(&src, 40_000, &p));
    // Off, the parameters are the old ones: same bits as before the options existed.
    let plain = StretchParams::new(StretchMode::Tonal, 1.3, SR);
    let a = render(&src, 40_000, &plain);
    let b = render(
        &src,
        40_000,
        &StretchParams {
            formant_ratio: 1.7,
            ..plain
        },
    );
    assert_eq!(a, b);
}

#[test]
fn non_finite_input_gives_no_nan() {
    let mut src = voice(110.0, 1.0);
    src[1000] = f32::NAN;
    src[5000] = f32::INFINITY;
    src[7000] = f32::NEG_INFINITY;
    let out = render(
        &[src.clone(), src],
        40_000,
        &StretchParams {
            link_stereo: true,
            ..tonal(1.3, true, 1.2)
        },
    );
    assert!(out.iter().flatten().all(|v| v.is_finite()));
}

/// The phase (radians) of `x` at `freq` over the middle of the buffer.
fn phase_at(x: &[f32], freq: f64) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, s) in x.iter().enumerate().skip(8000).take(40_000) {
        let w = TAU * freq * i as f64 / f64::from(SR);
        re += f64::from(*s) * w.cos();
        im -= f64::from(*s) * w.sin();
    }
    im.atan2(re)
}

fn wrap(x: f64) -> f64 {
    x - TAU * (x / TAU).round()
}

#[test]
fn linked_phases_keep_the_inter_channel_phase_of_panned_sources() {
    // Three partials panned differently, each with its own left/right phase offset.
    let n = 60_000;
    let partials = [
        (330.0, 1.0, 0.0, 0.6),
        (700.0, 0.7, 0.9, 0.4),
        (1210.0, 0.5, -0.5, 1.0),
    ];
    let mut left = vec![0.0f32; n];
    let mut right = vec![0.0f32; n];
    for (f, gl, off, gr) in partials {
        for i in 0..n {
            let w = TAU * f * i as f64 / f64::from(SR);
            left[i] += (0.3 * gl * w.sin()) as f32;
            right[i] += (0.3 * gr * (w + off).sin()) as f32;
        }
    }
    let src = [left, right];
    let p = StretchParams {
        link_stereo: true,
        ..StretchParams::new(StretchMode::Tonal, 1.0, SR)
    };
    let refs: Vec<&[f32]> = src.iter().map(Vec::as_slice).collect();
    let out = stretch(&refs, &TimeMap::linear(0.6), 60_000, &p, &never).unwrap();
    for (f, _, off, _) in partials {
        let d = wrap(phase_at(&out[1], f) - phase_at(&out[0], f));
        assert!(wrap(d - off).abs() < 0.05, "{f} Hz: {d} against {off}");
    }
}
