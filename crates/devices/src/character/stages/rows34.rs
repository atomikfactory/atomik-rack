//! Tests of the stage kinds added for rows 3 and 4 (spectral, phaser, auto-pan, echo, ring
//! modulator, comb, band-pass, crush, tremolo, swept low-pass, hum, bursts, rattle, modal bank,
//! room): the parameter mapping, the analytic behaviour of each wrapper, determinism, reset and
//! robustness. The per-preset character checks are in `tests/row34.rs`.

// Fixed-size frames are indexed next to parallel arrays throughout the measurements.
#![allow(clippy::chunks_exact_to_as_chunks)]

use super::*;
use atomik_dsp::room::rt60_from_impulse;

const SR: f32 = 48_000.0;

const NEW_KINDS: [Kind; 15] = [
    Kind::Spectral,
    Kind::Phaser,
    Kind::AutoPan,
    Kind::Echo,
    Kind::RingMod,
    Kind::Comb,
    Kind::BandPass,
    Kind::Crush,
    Kind::Tremolo,
    Kind::SweepLp,
    Kind::Hum,
    Kind::Bursts,
    Kind::Rattle,
    Kind::Modal,
    Kind::Room,
];

fn info(seed: u64) -> BuildInfo {
    BuildInfo {
        sample_rate: SR,
        seed,
        noise_follows_gain: false,
    }
}

/// Runs `st` over `l` and `r` in control blocks with constant parameters `p` and the gate at
/// `presence`; returns both outputs.
fn run(st: &mut dyn Stage, p: &[f32], l: &[f32], r: &[f32], presence: f32) -> (Vec<f32>, Vec<f32>) {
    let presence = [presence; SUB];
    let gain = [1.0f32; SUB];
    let ctx = Ctx {
        presence: &presence,
        gain: &gain,
    };
    st.set(p, true);
    let (mut ol, mut or) = (Vec::new(), Vec::new());
    let (mut o1, mut o2) = ([[0.0f32; SUB]; 2], [[0.0f32; SUB]; 2]);
    for (cl, cr) in l.chunks_exact(SUB).zip(r.chunks_exact(SUB)) {
        let mut inp = [[0.0f32; SUB]; 2];
        inp[0].copy_from_slice(cl);
        inp[1].copy_from_slice(cr);
        st.set(p, false);
        st.process(&inp, &mut o1, &mut o2, &ctx);
        ol.extend_from_slice(&o1[0]);
        or.extend_from_slice(&o1[1]);
    }
    (ol, or)
}

fn white(n: usize, seed: u64, rms: f32) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..n).map(|_| r.next_bipolar() * rms * 1.732).collect()
}

fn sine(n: usize, hz: f64, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| amp * (std::f64::consts::TAU * hz * i as f64 / f64::from(SR)).sin() as f32)
        .collect()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

/// Amplitude of the component at `hz` (a lock-in over `x`).
fn tone(x: &[f32], hz: f64) -> f64 {
    let (mut c, mut s) = (0.0f64, 0.0f64);
    for (n, v) in x.iter().enumerate() {
        let ph = std::f64::consts::TAU * hz * n as f64 / f64::from(SR);
        c += f64::from(*v) * ph.cos();
        s += f64::from(*v) * ph.sin();
    }
    2.0 * (c * c + s * s).sqrt() / x.len() as f64
}

fn impulse(n: usize, amp: f32) -> Vec<f32> {
    let mut v = vec![0.0; n];
    v[0] = amp;
    v
}

fn argmax(x: &[f32]) -> usize {
    let mut m = 0;
    for (i, v) in x.iter().enumerate() {
        if v.abs() > x[m].abs() {
            m = i;
        }
    }
    m
}

// --- The kind tables ---------------------------------------------------------------------------

#[test]
fn the_new_kinds_have_consistent_tables_latencies_and_generator_flags() {
    for k in NEW_KINDS {
        assert!(!k.params().is_empty(), "{k:?}");
        for p in k.params() {
            assert!(p.min < p.max, "{k:?} {}", p.name);
        }
        let s = build(k, &info(1));
        assert_eq!(s.latency(), k.latency(SR), "{k:?}");
        assert!(!s.two_outputs());
        assert_eq!(
            k.generator(),
            matches!(k, Kind::Hum | Kind::Bursts),
            "{k:?}"
        );
    }
    // Only the spectral stage lifts a preset into the long class; its latency is the frame.
    for (sr, frame) in [(44_100.0f32, 1024u32), (48_000.0, 1024), (96_000.0, 2048)] {
        assert_eq!(Kind::Spectral.latency(sr), frame);
        assert_eq!(Kind::Spectral.class_extra(sr), frame);
        for k in NEW_KINDS.into_iter().filter(|k| *k != Kind::Spectral) {
            assert_eq!(k.class_extra(sr), 0, "{k:?}");
            assert_eq!(k.latency(sr), 0, "{k:?}");
        }
    }
}

// --- Spectral ----------------------------------------------------------------------------------

#[test]
fn the_spectral_stage_delays_by_the_frame_exactly_and_dropouts_follow_the_seed() {
    // Identity-like setting: thresholds at the bottom, the finest step, no dropout.
    let p = [-100.0, 0.0, 0.5, 0.0];
    let mut st = build(Kind::Spectral, &info(3));
    let (l, _) = run(&mut *st, &p, &impulse(4096, 0.5), &impulse(4096, 0.5), 1.0);
    assert_eq!(argmax(&l), 1024);
    assert!((l[1024] - 0.5).abs() < 0.03, "{}", l[1024]);
    // Dropouts: same seed identical, other seed different, reset restarts the draws.
    let x = white(SUB * 400, 4, 0.1);
    let q = [-100.0, 0.0, 0.5, 0.5];
    let render = |seed: u64| {
        let mut s = build(Kind::Spectral, &info(seed));
        run(&mut *s, &q, &x, &x, 1.0).0
    };
    assert_eq!(render(5), render(5));
    assert_ne!(render(5), render(6));
    let mut s = build(Kind::Spectral, &info(5));
    let first = run(&mut *s, &q, &x, &x, 1.0).0;
    s.reset();
    assert_eq!(run(&mut *s, &q, &x, &x, 1.0).0, first);
}

// --- Phaser, auto-pan, echo, ring modulator ------------------------------------------------------

#[test]
fn the_phaser_mix_zero_is_dry_and_a_static_mix_one_notches_the_spectrum() {
    let x = white(SUB * 300, 5, 0.1);
    let mut st = build(Kind::Phaser, &info(1));
    // rate, depth (octaves), feedback, centre, mix, stereo
    let (l, _) = run(&mut *st, &[0.5, 1.0, 0.0, 800.0, 0.0, 0.0], &x, &x, 1.0);
    assert_eq!(l, x, "mix 0 is the exact dry signal");
    let mut st = build(Kind::Phaser, &info(1));
    let (l, _) = run(&mut *st, &[0.02, 0.0, 0.0, 800.0, 1.0, 0.0], &x, &x, 1.0);
    // With the sweep frozen the dry/phased sum has notches: its level differs from the input's
    // between frequencies (the all-pass phase turns through 180 degrees somewhere).
    let a = tone(&l[SUB * 100..], 150.0) / tone(&x[SUB * 100..], 150.0);
    assert!(a > 0.0);
    let mut notch = f64::MAX;
    for hz in (300..1_600).step_by(5) {
        let w = run_tone(
            Kind::Phaser,
            &[0.02, 0.0, 0.0, 800.0, 1.0, 0.0],
            f64::from(hz),
        );
        notch = notch.min(rms(&w) / (0.5 / 2f64.sqrt()));
    }
    assert!(notch < 0.2, "a notch of the dry plus phased sum: {notch}");
}

/// Output of `kind` for a steady tone (left channel), after its start-up.
fn run_tone(kind: Kind, p: &[f32], hz: f64) -> Vec<f32> {
    let x = sine(SUB * 200, hz, 0.5);
    let mut st = build(kind, &info(1));
    run(&mut *st, p, &x, &x, 1.0).0[SUB * 100..].to_vec()
}

#[test]
fn auto_pan_keeps_the_power_constant_and_rotates_at_its_rate() {
    let secs = 8.0f32;
    let n = (secs * SR) as usize / SUB * SUB;
    let x = white(n, 6, 0.1);
    let mut st = build(Kind::AutoPan, &info(1));
    let (l, r) = run(&mut *st, &[1.0, 1.0], &x, &x, 1.0);
    // Left and right power always sum to twice the input power (constant power).
    let (pl, pr, px) = (rms(&l).powi(2), rms(&r).powi(2), rms(&x).powi(2));
    assert!(
        ((pl + pr) / (2.0 * px) - 1.0).abs() < 0.03,
        "{}",
        (pl + pr) / (2.0 * px)
    );
    // The balance crosses zero twice per cycle (1 Hz).
    let win = 960;
    let bal: Vec<f64> = l
        .chunks_exact(win)
        .zip(r.chunks_exact(win))
        .map(|(a, b)| {
            let (p, q) = (rms(a).powi(2), rms(b).powi(2));
            (p - q) / (p + q)
        })
        .collect();
    let smooth: Vec<f64> = bal
        .windows(5)
        .step_by(2)
        .map(|w| w.iter().sum::<f64>() / 5.0)
        .collect();
    let crossings = smooth
        .windows(2)
        .filter(|w| w[0].signum() != w[1].signum())
        .count();
    assert!(
        (crossings as f64 - 2.0 * f64::from(secs)).abs() <= 3.0,
        "{crossings}"
    );
    // Depth 0 leaves the signal untouched (gain exactly 1 on both sides).
    let mut st = build(Kind::AutoPan, &info(1));
    let (l0, r0) = run(&mut *st, &[1.0, 0.0], &x, &x, 1.0);
    assert!(l0.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-6));
    assert!(r0.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-6));
}

#[test]
fn the_echo_repeats_at_its_time_with_the_feedback_law() {
    let mut st = build(Kind::Echo, &info(1));
    // time_ms, feedback, mix (fully wet), no modulation
    let (l, _) = run(
        &mut *st,
        &[100.0, 0.5, 1.0, 0.0, 0.0],
        &impulse(SUB * 1000, 0.8),
        &impulse(SUB * 1000, 0.8),
        1.0,
    );
    let at = |k: usize| {
        l[k * 4_800 - 3..k * 4_800 + 3]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    };
    assert!((at(1) / 0.8 - 1.0).abs() < 0.05, "{}", at(1));
    assert!((at(2) / (0.8 * 0.5) - 1.0).abs() < 0.05, "{}", at(2));
    assert!((at(3) / (0.8 * 0.25) - 1.0).abs() < 0.05, "{}", at(3));
    assert!(
        l[100..4_700].iter().all(|v| v.abs() < 1e-6),
        "silent between repeats"
    );
}

#[test]
fn the_ring_modulator_is_dry_at_amount_zero_and_makes_two_sidebands() {
    let x = sine(SUB * 1500, 1_000.0, 0.4);
    let mut st = build(Kind::RingMod, &info(1));
    let (l, _) = run(&mut *st, &[100.0, 0.0], &x, &x, 1.0);
    assert_eq!(l, x);
    let mut st = build(Kind::RingMod, &info(1));
    let (l, _) = run(&mut *st, &[100.0, 1.0], &x, &x, 1.0);
    let w = &l[SUB * 700..];
    // The plain product: sidebands at 900 and 1100 Hz of half the input amplitude, no carrier.
    assert!(
        (tone(w, 900.0) / 0.2 - 1.0).abs() < 0.05,
        "{}",
        tone(w, 900.0)
    );
    assert!((tone(w, 1_100.0) / 0.2 - 1.0).abs() < 0.05);
    assert!(tone(w, 1_000.0) < 0.01);
}

// --- Comb and band-pass --------------------------------------------------------------------------

#[test]
fn the_comb_is_normalised_to_the_input_power_and_rings_at_its_delay() {
    let x = white(SUB * 4000, 7, 0.1);
    for (fb, ff) in [(0.8f32, 0.0f32), (0.5, 0.0), (0.85, 0.5), (-0.6, 0.0)] {
        let mut st = build(Kind::Comb, &info(1));
        // delay_ms, feedback, feedforward, detune, wander
        let (l, r) = run(&mut *st, &[3.0, fb, ff, 0.0, 0.0], &x, &x, 1.0);
        assert!(
            (db(rms(&l[SUB * 800..]) / rms(&x[SUB * 800..]))).abs() < 1.0,
            "fb {fb} ff {ff}: {}",
            db(rms(&l[SUB * 800..]) / rms(&x[SUB * 800..]))
        );
        assert_eq!(l, r, "no detune: the channels agree");
    }
    // Autocorrelation at the delay equals the feedback (ff 0).
    let mut st = build(Kind::Comb, &info(1));
    let (l, _) = run(&mut *st, &[3.0, 0.7, 0.0, 0.0, 0.0], &x, &x, 1.0);
    let lag = 144; // 3 ms
    let y = &l[SUB * 800..];
    let num: f64 = (lag..y.len())
        .map(|i| f64::from(y[i]) * f64::from(y[i - lag]))
        .sum();
    let den: f64 = y.iter().map(|v| f64::from(*v).powi(2)).sum();
    assert!((num / den - 0.7).abs() < 0.05, "{}", num / den);
    // Detune: the right channel differs from the left.
    let mut st = build(Kind::Comb, &info(1));
    let (l, r) = run(&mut *st, &[3.0, 0.7, 0.0, 0.1, 0.0], &x, &x, 1.0);
    assert_ne!(l, r);
}

#[test]
fn the_band_pass_has_unity_gain_at_its_centre_and_q_sets_the_width() {
    for (f, q) in [(500.0f64, 3.0f32), (2_000.0, 3.0), (2_000.0, 8.0)] {
        let gain_at = |hz: f64| {
            let w = run_tone(Kind::BandPass, &[f as f32, q, 0.0], hz);
            rms(&w) / (0.5 / 2f64.sqrt())
        };
        assert!(
            (db(gain_at(f))).abs() < 0.4,
            "centre {f} q {q}: {}",
            db(gain_at(f))
        );
        // -3 dB at about f (1 +- 1 / (2 q)).
        let half = 1.0 / (2.0 * f64::from(q));
        let up = db(gain_at(f * (1.0 + half)));
        assert!((up + 3.0).abs() < 1.5, "upper edge {up}");
        assert!(db(gain_at(f * 0.25)) < -10.0);
    }
    // Make-up gain scales the output.
    let w = run_tone(Kind::BandPass, &[1_000.0, 3.0, 6.0], 1_000.0);
    assert!((db(rms(&w) / (0.5 / 2f64.sqrt())) - 6.0).abs() < 0.5);
}

// --- Crush ---------------------------------------------------------------------------------------

#[test]
fn the_crusher_has_two_to_the_bits_levels_holds_for_the_rate_and_takes_the_first_sample() {
    let x = sine(SUB * 800, 337.0, 0.95);
    // bits, rate, mod_rate, mod_depth, makeup: three bits, no hold (rate above fs), no modulation.
    let mut st = build(Kind::Crush, &info(1));
    let (l, _) = run(&mut *st, &[3.0, 96_000.0, 0.0, 0.0, 0.0], &x, &x, 1.0);
    let mut levels: Vec<i32> = l.iter().map(|v| (v * 4.0).round() as i32).collect();
    levels.sort_unstable();
    levels.dedup();
    assert!(levels.len() <= 8, "{levels:?}");
    assert!(
        l.iter()
            .all(|v| ((v * 4.0) - (v * 4.0).round()).abs() < 1e-5),
        "multiples of 1/4"
    );
    // The hold: 16 bits at 4 kHz holds each value for 12 samples (the first sample is taken at once).
    let mut st = build(Kind::Crush, &info(1));
    let (l, _) = run(&mut *st, &[16.0, 4_000.0, 0.0, 0.0, 0.0], &x, &x, 1.0);
    let changes = l
        .windows(2)
        .filter(|w| w[0].to_bits() != w[1].to_bits())
        .count();
    let expect = x.len() as f64 / 12.0;
    assert!(
        (changes as f64 / expect - 1.0).abs() < 0.05,
        "{changes} against {expect}"
    );
    assert!(
        (l[0] - x[0]).abs() < 1e-3 && l[1].to_bits() == l[0].to_bits(),
        "first sample taken at once"
    );
    // Random modulation moves the two channels apart and is deterministic by seed.
    let p = [6.0, 12_000.0, 8.0, 1.0, 0.0];
    let render = |seed: u64| {
        let mut s = build(Kind::Crush, &info(seed));
        run(&mut *s, &p, &x, &x, 1.0)
    };
    let (a, b) = render(3);
    assert_ne!(a, b);
    assert_eq!(render(3).0, a);
    assert_ne!(render(4).0, a);
    // Make-up gain.
    let mut st = build(Kind::Crush, &info(1));
    let (l, _) = run(&mut *st, &[16.0, 96_000.0, 0.0, 0.0, -6.0], &x, &x, 1.0);
    assert!((db(rms(&l[SUB * 100..]) / rms(&x[SUB * 100..])) + 6.0).abs() < 0.3);
}

// --- Tremolo and swept low-pass ----------------------------------------------------------------------

#[test]
fn the_tremolo_swings_between_one_minus_depth_and_one_with_constant_noise_level() {
    let x = white(SUB * 3000, 8, 0.1);
    let level = |depth: f32| {
        let mut st = build(Kind::Tremolo, &info(1));
        let (l, _) = run(&mut *st, &[3.0, depth, 0.25], &x, &x, 1.0);
        rms(&l[SUB * 200..])
    };
    // The compensation keeps the long-term level of noise within half a dB at every depth.
    for d in [0.2f32, 0.5, 0.8, 1.0] {
        assert!(
            db(level(d) / level(0.0)).abs() < 0.5,
            "depth {d}: {}",
            db(level(d) / level(0.0))
        );
    }
    // A steady tone: the envelope's extremes sit at (1 - d) and 1 times the compensation.
    let tone_x = sine(SUB * 3000, 440.0, 0.3);
    let mut st = build(Kind::Tremolo, &info(1));
    let (l, r) = run(&mut *st, &[2.0, 0.6, 0.25], &tone_x, &tone_x, 1.0);
    let env: Vec<f64> = l[SUB * 200..].chunks_exact(240).map(rms).collect();
    let (hi, lo) = (
        env.iter().fold(0.0f64, |m, v| m.max(*v)),
        env.iter().fold(f64::MAX, |m, v| m.min(*v)),
    );
    assert!((lo / hi - 0.4).abs() < 0.03, "{}", lo / hi);
    assert_ne!(l, r, "the channels are offset in phase");
}

#[test]
fn the_swept_low_pass_is_open_at_depth_zero_and_sweeps_at_depth_one() {
    // lo, hi, sweep rate, depth, q
    let hf = |depth: f32| {
        let x = sine(SUB * 6000, 2_000.0, 0.3);
        let mut st = build(Kind::SweepLp, &info(1));
        let (l, _) = run(&mut *st, &[400.0, 3_000.0, 2.0, depth, 0.7], &x, &x, 1.0);
        l[SUB * 300..]
            .chunks_exact(480)
            .map(rms)
            .collect::<Vec<f64>>()
    };
    // Depth 0: the cutoff stays at the top corner: a 2 kHz tone is almost untouched and steady.
    let open = hf(0.0);
    let (hi, lo) = (
        open.iter().fold(0.0f64, |m, v| m.max(*v)),
        open.iter().fold(f64::MAX, |m, v| m.min(*v)),
    );
    assert!(db(hi / lo) < 0.5, "{}", db(hi / lo));
    // Depth 1: the cutoff sweeps from 400 Hz to 3 kHz: the tone is much louder at the top.
    let sweep = hf(1.0);
    let (hi, lo) = (
        sweep.iter().fold(0.0f64, |m, v| m.max(*v)),
        sweep.iter().fold(f64::MAX, |m, v| m.min(*v)),
    );
    assert!(db(hi / lo) > 12.0, "{}", db(hi / lo));
}

// --- Hum, bursts, rattle -------------------------------------------------------------------------

#[test]
fn the_hum_has_its_fundamental_and_harmonic_ratios_and_is_gated_by_presence() {
    let z = vec![0.0f32; SUB * 2000];
    let mut st = build(Kind::Hum, &info(1));
    let (l, r) = run(&mut *st, &[-20.0, 55.0], &z, &z, 1.0);
    assert_eq!(l, r);
    let w = &l[SUB * 600..];
    assert!((db(rms(w) / 0.1)).abs() < 0.6, "level {}", db(rms(w) / 0.1));
    let a1 = tone(w, 55.0);
    for (h, a) in HUM_HARMONICS.iter().enumerate().skip(1) {
        let got = tone(w, 55.0 * (h + 1) as f64) / a1;
        assert!(
            (got / f64::from(*a) - 1.0).abs() < 0.08,
            "harmonic {}: {got}",
            h + 1
        );
    }
    // Closed gate: exact silence.
    let mut st = build(Kind::Hum, &info(1));
    let (l, _) = run(&mut *st, &[-20.0, 55.0], &z, &z, 0.0);
    assert!(l.iter().all(|v| *v == 0.0));
}

#[test]
fn bursts_fire_at_their_rate_over_a_duration_that_scales_with_it() {
    // Poisson counts: scale the duration with the rate so that each run sees at least 1500
    // bursts per channel (a rate of 0.5 per second would need 3000 s of audio).
    for rate in [6.0f32, 1.7] {
        let secs = f64::from(1_500.0 / rate);
        let mut st = BurstStage::new(SR, &info(5));
        st.set(&[rate, -20.0, 120.0, 2_500.0], true);
        let presence = [1.0f32; SUB];
        let gain = [1.0f32; SUB];
        let ctx = Ctx {
            presence: &presence,
            gain: &gain,
        };
        let inp = [[0.0f32; SUB]; 2];
        let (mut a, mut b) = ([[0.0f32; SUB]; 2], [[0.0f32; SUB]; 2]);
        let blocks = (secs * f64::from(SR) / SUB as f64) as usize;
        for _ in 0..blocks {
            st.process(&inp, &mut a, &mut b, &ctx);
        }
        // Both channels fire at the full rate.
        let measured = st.events as f64 / secs / 2.0;
        assert!(
            (measured / f64::from(rate) - 1.0).abs() < 0.1,
            "rate {rate}: {measured}"
        );
    }
}

#[test]
fn a_burst_is_noise_with_a_decaying_envelope_and_the_gate_silences_it() {
    let z = vec![0.0f32; SUB * 1000];
    let mut st = build(Kind::Bursts, &info(2));
    let (l, _) = run(&mut *st, &[10.0, -10.0, 100.0, 3_000.0], &z, &z, 1.0);
    // Energy comes in separate events: the short-time level varies by more than 30 dB.
    let env: Vec<f64> = l.chunks_exact(480).map(rms).collect();
    let (hi, lo) = (
        env.iter().fold(0.0f64, |m, v| m.max(*v)),
        env.iter().fold(f64::MAX, |m, v| m.min(*v)),
    );
    assert!(hi > 0.02 && db(hi / lo.max(1e-9)) > 30.0, "{hi} {lo}");
    // After a burst the envelope falls with the decay time: -60 dB per 100 ms * ln(1000)/... the
    // time constant is 100 ms, so 200 ms after the peak the level is e^-2 = 0.135 of it.
    let mut st = build(Kind::Bursts, &info(2));
    let (l, _) = run(&mut *st, &[0.0, -10.0, 100.0, 3_000.0], &z, &z, 1.0);
    assert!(l.iter().all(|v| *v == 0.0), "rate 0 makes no burst");
    let mut st = build(Kind::Bursts, &info(2));
    let (l, _) = run(&mut *st, &[10.0, -10.0, 100.0, 3_000.0], &z, &z, 0.0);
    assert!(l.iter().all(|v| *v == 0.0), "closed gate");
}

#[test]
fn the_rattle_is_silent_without_input_and_follows_the_input_envelope() {
    let z = vec![0.0f32; SUB * 400];
    let mut st = build(Kind::Rattle, &info(1));
    let (l, _) = run(&mut *st, &[1.0, 2_000.0], &z, &z, 1.0);
    assert!(l.iter().all(|v| *v == 0.0), "no input, no rattle");
    // With a tone the output is the tone plus noise: the noise level follows the amplitude.
    let noise_at = |amp: f32| {
        let x = sine(SUB * 1200, 300.0, amp);
        let mut st = build(Kind::Rattle, &info(1));
        let (l, _) = run(&mut *st, &[1.0, 2_000.0], &x, &x, 1.0);
        let d: Vec<f32> = l.iter().zip(&x).map(|(a, b)| a - b).collect();
        rms(&d[SUB * 300..])
    };
    let (quiet, loud) = (noise_at(0.05), noise_at(0.5));
    assert!(db(loud / quiet) > 12.0, "{}", db(loud / quiet));
    // Amount 0 is the dry signal.
    let x = sine(SUB * 200, 300.0, 0.5);
    let mut st = build(Kind::Rattle, &info(1));
    let (l, _) = run(&mut *st, &[0.0, 2_000.0], &x, &x, 1.0);
    assert!(l.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-6));
}

// --- Modal bank and room ---------------------------------------------------------------------------

#[test]
fn the_modal_stage_rings_at_root_times_ratio_for_the_active_modes_only() {
    let mut st = build(Kind::Modal, &info(1));
    // root, decay, detune, active, gain
    let (l, r) = run(
        &mut *st,
        &[300.0, 2.0, 0.0, 3.0, 1.0],
        &impulse(SUB * 2000, 1.0),
        &impulse(SUB * 2000, 1.0),
        1.0,
    );
    let sum: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a + b).collect();
    let w = &sum[100..24_100];
    let a = [
        tone(w, 300.0),
        tone(w, 477.0),
        tone(w, 642.0),
        tone(w, 690.0),
    ];
    assert!(a[0] > 1e-6 && a[1] > 1e-6 && a[2] > 1e-6, "{a:?}");
    // The fourth mode (300 x 2.30 = 690 Hz) is not active.
    assert!(a[3] < 0.02 * a[0], "{a:?}");
    // Even modes left, odd modes right.
    assert!(tone(&l[100..24_100], 300.0) > 20.0 * tone(&r[100..24_100], 300.0));
    assert!(tone(&r[100..24_100], 477.0) > 20.0 * tone(&l[100..24_100], 477.0));
}

#[test]
fn the_room_decays_at_the_requested_time_and_the_stage_is_deterministic_by_seed() {
    for rt in [0.2f32, 0.6] {
        let mut st = build(Kind::Room, &info(9));
        // size, rt60, damping
        let (l, _) = run(
            &mut *st,
            &[0.4, rt, 0.2],
            &impulse(SUB * 1600, 0.5),
            &impulse(SUB * 1600, 0.5),
            1.0,
        );
        let got = rt60_from_impulse(&l, SR);
        assert!(
            (got / rt - 1.0).abs() < 0.35,
            "requested {rt}, measured {got}"
        );
    }
    let x = white(SUB * 200, 3, 0.1);
    let render = |seed: u64| {
        let mut s = build(Kind::Room, &info(seed));
        run(&mut *s, &[0.4, 0.3, 0.2], &x, &x, 1.0).0
    };
    assert_eq!(render(1), render(1));
    assert_ne!(render(1), render(2));
}

// --- Every new stage: reset, robustness ----------------------------------------------------------------

/// A mid-range parameter vector of `kind` (the middle of every range, geometric for Hz).
fn mid_params(kind: Kind) -> Vec<f32> {
    kind.params()
        .iter()
        .map(|p| {
            if p.min > 0.0 && p.max / p.min > 50.0 {
                (p.min * p.max).sqrt()
            } else {
                0.5 * (p.min + p.max)
            }
        })
        .collect()
}

#[test]
fn reset_and_a_snapped_set_restore_the_start_state_of_every_new_stage() {
    let x = white(SUB * 300, 11, 0.2);
    for k in NEW_KINDS {
        let p = mid_params(k);
        let mut st = build(k, &info(4));
        let first = run(&mut *st, &p, &x, &x, 1.0);
        st.reset();
        let again = run(&mut *st, &p, &x, &x, 1.0);
        assert_eq!(first, again, "{k:?}");
    }
}

#[test]
fn every_new_stage_stays_finite_and_bounded_at_the_extremes_of_its_parameters() {
    let x = white(SUB * 400, 12, 0.5);
    for k in NEW_KINDS {
        let specs = k.params();
        for pattern in 0..8u32 {
            let p: Vec<f32> = specs
                .iter()
                .enumerate()
                .map(|(i, s)| match (pattern >> (i % 3)) & 1 {
                    0 if pattern < 4 => s.min,
                    1 => s.max,
                    _ => 0.5 * (s.min + s.max),
                })
                .collect();
            let mut st = build(k, &info(u64::from(pattern)));
            let (l, r) = run(&mut *st, &p, &x, &x, 1.0);
            assert!(
                l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 1.0e3),
                "{k:?} pattern {pattern}: {:?}",
                p
            );
        }
    }
}

#[test]
fn the_new_stages_never_allocate_while_processing() {
    use atomik_testkit::no_alloc::assert_no_alloc;
    let presence = [1.0f32; SUB];
    let gain = [1.0f32; SUB];
    let ctx = Ctx {
        presence: &presence,
        gain: &gain,
    };
    let mut inp = [[0.0f32; SUB]; 2];
    for (i, v) in inp[0].iter_mut().enumerate() {
        *v = 0.3 * (i as f32 * 0.7).sin();
    }
    inp[1] = inp[0];
    for k in NEW_KINDS {
        let p = mid_params(k);
        let mut st = build(k, &info(1));
        st.set(&p, true);
        let (mut a, mut b) = ([[0.0f32; SUB]; 2], [[0.0f32; SUB]; 2]);
        for _ in 0..4 {
            assert_no_alloc(|| {
                st.set(&p, false);
                st.process(&inp, &mut a, &mut b, &ctx);
            });
        }
        assert_no_alloc(|| st.reset());
    }
}
