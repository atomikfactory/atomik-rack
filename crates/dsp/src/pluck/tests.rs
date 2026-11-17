//! Tests of the plucked string: the arctangent, the tuning against the tuner's detector and an
//! exact spectral fit, the decay, the glide, the pick, the body and the numerics.

use super::body::{BODY_NAMES, Body};
use super::*;
use crate::test_util::{db, max_step, noise, rms};
use crate::tuner;

fn key_hz(key: i32) -> f64 {
    440.0 * ((f64::from(key) - 69.0) / 12.0).exp2()
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

fn tone(decay: f32, brightness: f32, stiffness: f32) -> StringTone {
    StringTone {
        decay_s: decay,
        brightness,
        stiffness,
    }
}

fn plucked(sr: f32, hz: f32, t: &StringTone, position: f32, seed: u64, n: usize) -> Vec<f32> {
    let mut s = PluckString::new(sr);
    s.set_pitch(hz, t, 0);
    let mut p = Pick::new(sr);
    p.start(
        &PickParams {
            period: s.period(),
            cutoff_hz: 6_000.0,
            position,
            level: 0.3,
        },
        seed,
    );
    (0..n).map(|_| s.tick(p.sample())).collect()
}

/// The frequency of the strongest component near `guess`: the maximum of the Hann-windowed
/// spectrum, found by golden-section search over plus or minus 1.5 %.
fn fit_hz(x: &[f32], sr: f64, guess: f64) -> f64 {
    let n = x.len();
    let win: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect();
    let mag = |f: f64| -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, s) in x.iter().enumerate() {
            let a = std::f64::consts::TAU * f * i as f64 / sr;
            let v = f64::from(*s) * win[i];
            re += v * a.cos();
            im += v * a.sin();
        }
        re * re + im * im
    };
    let (mut lo, mut hi) = (guess * 0.985, guess * 1.015);
    let phi = 0.618_033_988_749_895;
    let (mut a, mut b) = (hi - phi * (hi - lo), lo + phi * (hi - lo));
    let (mut fa, mut fb) = (mag(a), mag(b));
    for _ in 0..40 {
        if fa > fb {
            hi = b;
            b = a;
            fb = fa;
            a = hi - phi * (hi - lo);
            fa = mag(a);
        } else {
            lo = a;
            a = b;
            fa = fb;
            b = lo + phi * (hi - lo);
            fb = mag(b);
        }
    }
    0.5 * (lo + hi)
}

#[test]
fn the_arctangent_matches_the_library_one() {
    for i in -200..=200 {
        let x = f64::from(i) * 0.37;
        assert!((atan_det(x) - x.atan()).abs() < 1e-11, "{x}");
    }
    for (y, x) in [
        (1.0, 1.0),
        (1.0, -1.0),
        (-1.0, -1.0),
        (-1.0, 1.0),
        (0.0, -1.0),
        (2.0, 0.0),
    ] {
        assert!(
            (atan2_det(y, x) - f64::atan2(y, x)).abs() < 1e-11,
            "{y} {x}"
        );
    }
    assert_eq!(atan_det(f64::NAN), 0.0);
    assert!((atan_det(f64::INFINITY) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
}

#[test]
fn the_phase_delays_match_a_direct_evaluation() {
    // Evaluate the filters on the unit circle with complex arithmetic.
    let eval = |num: [f64; 2], den: [f64; 2], w: f64| -> f64 {
        let (c, s) = (w.cos(), w.sin());
        let n = (num[0] + num[1] * c, -num[1] * s);
        let d = (den[0] + den[1] * c, -den[1] * s);
        let arg = n.1.atan2(n.0) - d.1.atan2(d.0);
        -arg / w
    };
    for cycles in [0.001, 0.01, 0.05, 0.15] {
        let w = std::f64::consts::TAU * cycles;
        for p in [0.0, 0.3, 0.9] {
            let want = eval([1.0 - p, 0.0], [1.0, -p], w);
            assert!(
                (lowpass_delay(p, cycles) - want).abs() < 1e-9,
                "lp {p} {cycles}"
            );
        }
        for a in [-0.5, -0.1, 0.0, 0.4] {
            let want = eval([a, 1.0], [1.0, a], w);
            assert!(
                (allpass_delay(a, cycles) - want).abs() < 1e-9,
                "ap {a} {cycles}"
            );
        }
    }
}

#[test]
fn the_pitch_is_within_a_cent_over_the_keyboard_at_every_sample_rate() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        for key in 28..=100 {
            // Every key at 48 kHz, every fourth at the others (a fit is cheap but not free).
            if sr.to_bits() != 48_000.0f32.to_bits() && key % 4 != 0 {
                continue;
            }
            let want = key_hz(key);
            for (b, st) in [(0.2f32, 0.0f32), (0.9, 0.0), (0.5, 0.5)] {
                let x = plucked(sr, want as f32, &tone(10.0, b, st), 0.2, 5, 40_000);
                // Skip the attack, fit the rest.
                let got = fit_hz(&x[8_000..8_000 + 16_384], f64::from(sr), want);
                let off = cents(got, want).abs();
                assert!(
                    off < 1.0,
                    "key {key} at {sr}: {off:.3} cents (b {b}, stiff {st})"
                );
            }
        }
    }
}

#[test]
fn the_tuner_detector_reads_the_strings_in_tune() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        for key in (28..=91).step_by(3) {
            let want = key_hz(key);
            let x = plucked(sr, want as f32, &tone(10.0, 0.6, 0.1), 0.2, 9, 60_000);
            let frames = tuner::frames_wanted(sr);
            let window = &x[x.len() - frames..];
            let pitch = tuner::detect(window, sr).unwrap_or_else(|| panic!("key {key} at {sr}"));
            let off = cents(f64::from(pitch.hz), want).abs();
            assert!(
                off < 1.0,
                "key {key} at {sr}: the tuner reads {off:.3} cents off"
            );
        }
    }
}

#[test]
fn the_decay_time_is_the_time_to_fall_by_sixty_decibels() {
    for sr in [44_100.0f32, 96_000.0] {
        for decay in [1.0f32, 3.0] {
            // Mid key, bright, so the low-pass takes almost nothing at the fundamental.
            let hz = 164.81f32;
            let x = plucked(sr, hz, &tone(decay, 0.9, 0.0), 0.2, 3, (5.0 * sr) as usize);
            let win = (0.2 * sr) as usize;
            let level = |t: f32| {
                let i = (t * sr) as usize;
                rms(&x[i..i + win])
            };
            // The window levels include the harmonics, which fall faster, so measure at the
            // fundamental only: the 60 dB claim is about it. Fit by the amplitude of a sine.
            let amp = |t: f32| -> f64 {
                let i = (t * sr) as usize;
                let n = win;
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (k, s) in x[i..i + n].iter().enumerate() {
                    let a = std::f64::consts::TAU * f64::from(hz) * k as f64 / f64::from(sr);
                    re += f64::from(*s) * a.cos();
                    im += f64::from(*s) * a.sin();
                }
                2.0 * (re * re + im * im).sqrt() / n as f64
            };
            assert!(level(0.1) > 0.0);
            let (t0, t1) = (0.3f32, 0.3 + decay * 0.5);
            let fall = db(amp(t1) / amp(t0)).abs();
            let want = 60.0 * f64::from(t1 - t0) / f64::from(decay);
            assert!(
                (fall - want).abs() < want * 0.2,
                "decay {decay} at {sr}: fell {fall:.1} dB, wanted {want:.1}"
            );
        }
    }
}

#[test]
fn a_dull_string_loses_its_high_partials_first() {
    let sr = 48_000.0;
    let bright = plucked(sr, 110.0, &tone(2.0, 1.0, 0.0), 0.2, 4, 48_000);
    let dull = plucked(sr, 110.0, &tone(2.0, 0.0, 0.0), 0.2, 4, 48_000);
    let hi = |x: &[f32]| -> f64 {
        // Energy above 2 kHz from a first difference twice (a crude high-pass).
        let d: Vec<f32> = x.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
        f64::from(rms(&d[12_000..]))
    };
    assert!(
        hi(&bright) > 4.0 * hi(&dull),
        "{} {}",
        hi(&bright),
        hi(&dull)
    );
}

#[test]
fn stiffness_stretches_the_partials_sharp() {
    let sr = 48_000.0f32;
    let hz = 110.0f64;
    let partial = |stiff: f32| -> f64 {
        let x = plucked(sr, hz as f32, &tone(10.0, 0.9, stiff), 0.1, 6, 60_000);
        fit_hz(&x[10_000..10_000 + 32_768], f64::from(sr), hz * 8.0)
    };
    let (plain, stiff) = (partial(0.0), partial(0.9));
    // Within the harmonic series at zero, clearly sharp when stiff.
    assert!(
        cents(plain, hz * 8.0).abs() < 10.0,
        "{}",
        cents(plain, hz * 8.0)
    );
    assert!(cents(stiff, plain) > 5.0, "{}", cents(stiff, plain));
    // The fundamental stays in tune with any stiffness.
    let x = plucked(sr, hz as f32, &tone(10.0, 0.9, 0.9), 0.1, 6, 60_000);
    let f0 = fit_hz(&x[10_000..10_000 + 32_768], f64::from(sr), hz);
    assert!(cents(f0, hz).abs() < 1.0);
}

#[test]
fn a_new_pitch_glides_without_a_click() {
    let sr = 48_000.0f32;
    let t = tone(6.0, 0.7, 0.2);
    let mut s = PluckString::new(sr);
    let mut p = Pick::new(sr);
    s.set_pitch(196.0, &t, 0);
    p.start(
        &PickParams {
            period: s.period(),
            cutoff_hz: 4_000.0,
            position: 0.2,
            level: 0.3,
        },
        1,
    );
    let mut x = Vec::new();
    for n in 0..48_000 {
        // A two-semitone bend over 100 ms in control steps of 16 samples.
        if n % 16 == 0 {
            let k = ((n as f32 - 12_000.0) / 4_800.0).clamp(0.0, 1.0);
            s.set_pitch(196.0 * 2f32.powf(2.0 * k / 12.0), &t, 16);
        }
        x.push(s.tick(p.sample()));
    }
    // The same note with no bend is the reference: no step of the bent one is larger than the
    // steps the plain note itself takes (a jump in the delay would be).
    let plain = plucked(sr, 196.0, &t, 0.2, 1, 48_000);
    let reference = max_step(&plain[12_000..18_000]);
    let bend = max_step(&x[12_000..18_000]);
    assert!(
        bend < reference * 1.5,
        "bend step {bend}, plain {reference}"
    );
    assert!(x.iter().all(|v| v.is_finite()));
}

#[test]
fn the_pick_position_cancels_the_partials_at_the_plucked_point() {
    // A period of exactly 100 samples: a pick a quarter of the way along cancels every fourth
    // partial.
    let sr = 48_000.0f32;
    let hz = 480.0f32;
    let comb = |pos: f32| -> (f64, f64) {
        let x = plucked(sr, hz, &tone(4.0, 0.9, 0.0), pos, 11, 40_000);
        let w = &x[2_000..2_000 + 16_384];
        let amp = |f: f64| -> f64 {
            let f = fit_hz(w, f64::from(sr), f);
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (k, s) in w.iter().enumerate() {
                let a = std::f64::consts::TAU * f * k as f64 / f64::from(sr);
                re += f64::from(*s) * a.cos();
                im += f64::from(*s) * a.sin();
            }
            (re * re + im * im).sqrt()
        };
        (amp(f64::from(hz) * 3.0), amp(f64::from(hz) * 4.0))
    };
    let (three, four) = comb(0.25);
    assert!(db(four / three) < -20.0, "{} dB", db(four / three));
    // With no comb the fourth partial is as strong as its neighbours.
    let (three, four) = comb(0.0);
    assert!(db(four / three) > -12.0, "{} dB", db(four / three));
}

#[test]
fn the_pick_is_a_function_of_its_seed() {
    let t = tone(2.0, 0.5, 0.1);
    let a = plucked(48_000.0, 220.0, &t, 0.2, 1, 4_096);
    let b = plucked(48_000.0, 220.0, &t, 0.2, 1, 4_096);
    let c = plucked(48_000.0, 220.0, &t, 0.2, 2, 4_096);
    assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
    assert!(a != c);
}

#[test]
fn a_ringing_tail_ends_at_exact_zero_and_stays_finite() {
    let sr = 48_000.0f32;
    let x = plucked(sr, 330.0, &tone(0.3, 0.5, 0.3), 0.2, 8, 10 * 48_000);
    assert!(x.iter().all(|v| v.is_finite()));
    assert!(
        x[9 * 48_000..].iter().all(|v| *v == 0.0),
        "denormals are flushed to zero"
    );
}

#[test]
fn extreme_settings_stay_bounded() {
    for sr in [44_100.0f32, 96_000.0] {
        for hz in [
            0.0f32,
            1.0,
            20.0,
            5_000.0,
            30_000.0,
            f32::NAN,
            f32::INFINITY,
            -50.0,
        ] {
            for t in [
                tone(60.0, 1.0, 1.0),
                tone(0.0, 0.0, 0.0),
                tone(f32::NAN, f32::NAN, f32::NAN),
                tone(1e9, 5.0, -3.0),
            ] {
                let x = plucked(sr, hz, &t, 0.5, 3, 20_000);
                assert!(
                    x.iter().all(|v| v.is_finite() && v.abs() < 50.0),
                    "{hz} {t:?}"
                );
            }
        }
    }
}

#[test]
fn a_non_finite_excitation_clears_the_string() {
    let mut s = PluckString::new(48_000.0);
    s.set_pitch(220.0, &tone(2.0, 0.5, 0.0), 0);
    for _ in 0..100 {
        s.tick(0.1);
    }
    assert_eq!(s.tick(f32::NAN), 0.0);
    for _ in 0..1_000 {
        assert_eq!(s.tick(0.0), 0.0, "cleared");
    }
}

#[test]
fn the_pick_burst_has_the_asked_level_and_ends() {
    let sr = 48_000.0;
    let mut p = Pick::new(sr);
    p.start(
        &PickParams {
            period: 200.0,
            cutoff_hz: 8_000.0,
            position: 0.0,
            level: 0.5,
        },
        4,
    );
    let burst: Vec<f32> = (0..200).map(|_| p.sample()).collect();
    assert!((rms(&burst) - 0.5).abs() < 0.12, "{}", rms(&burst));
    for _ in 0..4 {
        p.sample();
    }
    assert!(!p.active());
    assert_eq!(p.sample(), 0.0);
}

#[test]
fn the_bodies_shape_the_spectrum_and_none_is_a_bypass() {
    let sr = 48_000.0f32;
    let src = noise(3, 96_000, 0.5);
    let mut none = Body::new(sr);
    none.set_kind(0);
    assert!(none.is_bypass());
    assert_eq!(none.process(0.25, -0.5, 1.0), (0.25, -0.5));
    let gain_at = |kind: usize, hz: f64| -> f64 {
        let mut b = Body::new(sr);
        b.set_kind(kind);
        let out: Vec<f32> = src.iter().map(|s| b.process(*s, *s, 1.0).0).collect();
        // Compare the energy in a third of an octave around `hz`.
        let band = |x: &[f32]| -> f64 {
            let n = 16_384;
            let spec = crate::test_util::spectrum(&x[8_192..8_192 + n]);
            let lo = (hz / 1.12 * n as f64 / f64::from(sr)) as usize;
            let hi = (hz * 1.12 * n as f64 / f64::from(sr)) as usize;
            spec[lo..=hi].iter().map(|v| v * v).sum::<f64>()
        };
        10.0 * (band(&out) / band(&src)).log10()
    };
    // The steel body rings at its air resonance and is flat-ish away from it; the resonator
    // lifts the upper mids; the electric body has no cavity but rolls off.
    assert!(gain_at(1, 100.0) > gain_at(1, 400.0) + 4.0);
    assert!(gain_at(9, 1_250.0) > gain_at(9, 300.0) + 5.0);
    assert!(gain_at(3, 12_000.0) < -6.0);
    for (kind, name) in BODY_NAMES.iter().enumerate() {
        let mut b = Body::new(sr);
        b.set_kind(kind);
        let out: Vec<f32> = src.iter().map(|s| b.process(*s, *s, 0.7).1).collect();
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 8.0), "{name}");
    }
    // Dry at a mix of zero, whatever the body.
    let mut b = Body::new(sr);
    b.set_kind(1);
    assert_eq!(b.process(0.3, 0.2, 0.0), (0.3, 0.2));
}
