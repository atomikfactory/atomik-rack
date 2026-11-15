//! Stereo tool tests, all through `DeviceHarness`.
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, sine, sweep_params_min_max, tone_amplitude};
use atomik_testkit::harness::DeviceHarness;

fn tool() -> DeviceHarness {
    harness(Box::new(StereoTool::new()))
}

fn noise(n: usize, seed: u32) -> f32 {
    let mut x = (n as u32).wrapping_mul(2_654_435_761).wrapping_add(seed);
    x ^= x >> 15;
    x = x.wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    (x as f32 / u32::MAX as f32) * 1.6 - 0.8
}

fn run(
    h: &mut DeviceHarness,
    l: impl Fn(usize) -> f32,
    r: impl Fn(usize) -> f32,
    batches: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut out_l, mut out_r) = (Vec::new(), Vec::new());
    for b in 0..batches {
        let mut xl = [0.0; BATCH];
        let mut xr = [0.0; BATCH];
        for i in 0..BATCH {
            xl[i] = l(b * BATCH + i);
            xr[i] = r(b * BATCH + i);
        }
        *h.input_mut("in_l") = xl;
        *h.input_mut("in_r") = xr;
        h.process();
        out_l.extend_from_slice(h.output("out_l"));
        out_r.extend_from_slice(h.output("out_r"));
    }
    (out_l, out_r)
}

fn last(v: &[f32]) -> f32 {
    v[v.len() - 1]
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "mode",
            "invert_l",
            "invert_r",
            "width",
            "low_bypass",
            "balance",
            "mono",
            "rotate",
            "rotate_hz",
            "bypass",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.stereo-tool");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn width_one_is_a_null() {
    for low in [20.0, 200.0] {
        let mut h = tool();
        h.set_param("low_bypass", low);
        let (l, r) = run(&mut h, |n| noise(n, 1), |n| noise(n, 2), 60);
        for n in 0..l.len() {
            assert_eq!(l[n].to_bits(), noise(n, 1).to_bits(), "low {low} at {n}");
            assert_eq!(r[n].to_bits(), noise(n, 2).to_bits(), "low {low} at {n}");
        }
    }
}

#[test]
fn polarity_inverts_one_channel_each() {
    let mut h = tool();
    h.set_param("invert_l", 1.0);
    let (l, r) = run(&mut h, |_| 0.4, |_| 0.4, 120);
    assert_eq!((last(&l), last(&r)), (-0.4, 0.4));
    h.set_param("invert_l", 0.0);
    h.set_param("invert_r", 1.0);
    let (l, r) = run(&mut h, |_| 0.4, |_| 0.4, 120);
    assert_eq!((last(&l), last(&r)), (0.4, -0.4));
}

#[test]
fn width_zero_and_mono_collapse_to_the_mid() {
    let mut h = tool();
    h.set_param("width", 0.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    assert!((last(&l) - 0.5).abs() < 1e-6 && (last(&r) - 0.5).abs() < 1e-6);
    let mut h = tool();
    h.set_param("mono", 1.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    assert!((last(&l) - 0.5).abs() < 1e-6 && (last(&r) - 0.5).abs() < 1e-6);
}

#[test]
fn mono_is_the_last_stage_so_the_output_is_mono_in_every_mode() {
    for mode in [0.0, 1.0, 2.0] {
        let mut h = tool();
        h.set_param("mode", mode);
        h.set_param("mono", 1.0);
        let (l, r) = run(&mut h, |n| noise(n, 3), |n| noise(n, 4), 40);
        assert!(
            l.iter().zip(&r).skip(BATCH * 5).all(|(a, b)| a == b),
            "mode {mode}: left and right differ"
        );
        // The output is not silent: the right channel is no longer lost in encode mode.
        assert!(l.iter().any(|x| x.abs() > 0.05), "mode {mode}: silent");
    }
    // Encode then mono: both channels carry (M + S) / 2 = L / 2 of the input.
    let mut h = tool();
    h.set_param("mode", 1.0);
    h.set_param("mono", 1.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    let want = 0.5 * (0.5 * (0.8 + 0.2) + 0.5 * (0.8 - 0.2));
    assert!((last(&l) - want).abs() < 1e-6 && (last(&r) - want).abs() < 1e-6);
}

#[test]
fn balance_lowers_the_far_side() {
    let mut h = tool();
    h.set_param("balance", -1.0);
    let (l, r) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!((last(&l), last(&r)), (0.5, 0.0));
}

#[test]
fn encode_then_decode_restores_the_signal() {
    let mut enc = tool();
    enc.set_param("mode", 1.0);
    let (m, s) = run(&mut enc, |n| noise(n, 7), |n| noise(n, 8), 20);
    for n in 0..m.len() {
        let (l, r) = (noise(n, 7), noise(n, 8));
        assert!((m[n] - 0.5 * (l + r)).abs() < 1e-6);
        assert!((s[n] - 0.5 * (l - r)).abs() < 1e-6);
    }
    let mut dec = tool();
    dec.set_param("mode", 2.0);
    let (l, r) = run(&mut dec, |n| m[n], |n| s[n], 20);
    for n in 0..l.len() {
        assert!((l[n] - noise(n, 7)).abs() < 1e-6);
        assert!((r[n] - noise(n, 8)).abs() < 1e-6);
    }
}

/// Amplitude of the `hz` tone on `out_l` after 0.5 s of settling.
fn left_amp(h: &mut DeviceHarness, hz: f64) -> f64 {
    let f = sine(hz, 0.5);
    let batches = (SR as usize).div_ceil(BATCH) + 100;
    let (l, _) = run(h, &f, |n| -f(n), batches);
    let start = 100 * BATCH;
    tone_amplitude(&l[start..start + SR as usize], hz, start)
}

#[test]
fn low_bypass_keeps_the_bass_wide_while_the_highs_collapse() {
    // A pure side signal (L = -R) at width 0.
    let mut full = tool();
    full.set_param("width", 0.0);
    assert!(left_amp(&mut full, 80.0) < 1e-4);
    assert!(left_amp(&mut full, 8_000.0) < 1e-4);

    let mut kept = tool();
    kept.set_param("width", 0.0);
    kept.set_param("low_bypass", 500.0);
    // 80 Hz is two octaves under the corner: almost untouched. 8 kHz is gone.
    let bass = left_amp(&mut kept, 80.0);
    assert!(bass > 0.45, "{bass}");
    assert!(left_amp(&mut kept, 8_000.0) < 1e-3);
}

#[test]
fn the_registered_device_bypasses_to_the_input() {
    let mut h = DeviceHarness::new((factory().create)(), SR);
    h.set_param("mono", 1.0);
    h.set_param("bypass", 1.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 80);
    assert_eq!((last(&l), last(&r)), (0.8, 0.2));
}

#[test]
fn params_at_their_limits_keep_the_output_finite() {
    let mut h = tool();
    *h.input_mut("in_l") = [0.9; BATCH];
    *h.input_mut("in_r") = [-0.3; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn non_finite_input_is_treated_as_silence() {
    let mut h = tool();
    h.set_param("width", 1.5);
    let (l, r) = run(&mut h, |_| f32::NAN, |_| f32::INFINITY, 4);
    assert!(l.iter().chain(&r).all(|x| *x == 0.0));
}

fn tone(hz: f32) -> impl Fn(usize) -> f32 {
    move |i| 0.5 * (2.0 * std::f32::consts::PI * hz * i as f32 / SR as f32).sin()
}

#[test]
fn rotation_at_zero_is_the_input_bit_for_bit() {
    let mut h = tool();
    h.set_param("rotate", 0.0);
    h.set_param("rotate_hz", 60.0);
    let (l, r) = run(&mut h, tone(55.0), |i| noise(i, 3), 40);
    let want_l: Vec<f32> = (0..l.len()).map(tone(55.0)).collect();
    let want_r: Vec<f32> = (0..r.len()).map(|i| noise(i, 3)).collect();
    assert_eq!((l, r), (want_l, want_r));
}

#[test]
fn rotation_at_180_is_a_polarity_inversion_of_both_channels() {
    let mut h = tool();
    h.set_param("rotate", 180.0);
    let (l, r) = run(&mut h, tone(55.0), tone(80.0), 20);
    for i in 0..l.len() {
        assert_eq!(l[i], -tone(55.0)(i));
        assert_eq!(r[i], -tone(80.0)(i));
    }
}

#[test]
fn rotation_turns_the_phase_and_keeps_the_level() {
    let mut h = tool();
    h.set_param("rotate", 90.0);
    h.set_param("rotate_hz", 100.0);
    let (l, _) = run(&mut h, tone(100.0), tone(100.0), 400);
    let n = l.len();
    // A sine turned by 90 degrees at its own frequency is a cosine of the same size.
    let want = |i: usize| -0.5 * (2.0 * std::f32::consts::PI * 100.0 * i as f32 / SR as f32).cos();
    let err = (n - 4_000..n)
        .map(|i| (l[i] - want(i)).abs())
        .fold(0.0f32, f32::max);
    assert!(err < 0.01, "{err}");
}

#[test]
fn rotation_agrees_with_the_analysis_rotator() {
    let mut h = tool();
    h.set_param("rotate", 70.0);
    h.set_param("rotate_hz", 150.0);
    let (l, _) = run(&mut h, |i| noise(i, 9), |i| noise(i, 9), 30);
    let mut r = atomik_dsp::PhaseRotator::new();
    r.set(70.0, 150.0, SR as f32);
    for (i, got) in l.iter().enumerate() {
        assert_eq!(*got, r.process(noise(i, 9)));
    }
}

// ----- regression guard: projects saved before the upgrade must sound bit-identical -----

fn fnv(h: &mut u64, v: f32) {
    for b in v.to_bits().to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Renders `batches` batches of noise; `setup` sets params before each batch runs. Returns the
/// FNV-1a hash of every output sample (left then right per batch).
fn render_hash(batches: usize, mut setup: impl FnMut(&mut DeviceHarness, usize)) -> u64 {
    let mut h = tool();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in 0..batches {
        let mut xl = [0.0f32; BATCH];
        let mut xr = [0.0f32; BATCH];
        for i in 0..BATCH {
            let n = b * BATCH + i;
            // A tone and noise, with the right channel partly correlated with the left.
            let t = (2.0 * std::f32::consts::PI * 110.0 * n as f32 / SR as f32).sin();
            xl[i] = 0.4 * t + 0.5 * noise(n, 21);
            xr[i] = 0.2 * t + 0.5 * noise(n, 22);
        }
        setup(&mut h, b);
        *h.input_mut("in_l") = xl;
        *h.input_mut("in_r") = xr;
        h.process();
        for key in ["out_l", "out_r"] {
            for v in h.output(key) {
                fnv(&mut hash, *v);
            }
        }
    }
    hash
}

#[test]
fn existing_projects_sound_bit_identical_after_the_upgrade() {
    // Defaults: the input passes untouched.
    let defaults = render_hash(200, |_, _| {});
    // Width without a split, with changes mid-ramp.
    let width = render_hash(400, |h, b| match b {
        0 => h.set_param("width", 0.0),
        100 => h.set_param("width", 2.0),
        200 => h.set_param("width", 0.7),
        203 => h.set_param("width", 1.4),
        300 => h.set_param("width", 1.0),
        _ => {}
    });
    // Width with the low band kept: several corners, a corner moved mid-run, width back to 1.
    let split = render_hash(500, |h, b| match b {
        0 => {
            h.set_param("low_bypass", 200.0);
            h.set_param("width", 0.0);
        }
        100 => h.set_param("width", 1.8),
        200 => h.set_param("low_bypass", 500.0),
        250 => h.set_param("low_bypass", 63.0),
        300 => h.set_param("width", 1.0),
        400 => h.set_param("width", 0.3),
        450 => h.set_param("low_bypass", 20.0),
        _ => {}
    });
    // Rotation, with polarity, balance and mono around it.
    let rotation = render_hash(400, |h, b| match b {
        0 => {
            h.set_param("rotate", 90.0);
            h.set_param("rotate_hz", 80.0);
        }
        100 => {
            h.set_param("rotate_hz", 300.0);
            h.set_param("invert_l", 1.0);
        }
        200 => {
            h.set_param("rotate", 180.0);
            h.set_param("balance", 0.6);
        }
        300 => {
            h.set_param("rotate", 35.0);
            h.set_param("mono", 1.0);
        }
        _ => {}
    });
    // The mid/side modes, with width and a split in front.
    let modes = render_hash(400, |h, b| match b {
        0 => {
            h.set_param("mode", 1.0);
            h.set_param("width", 1.3);
        }
        100 => h.set_param("mode", 2.0),
        200 => {
            h.set_param("mode", 0.0);
            h.set_param("low_bypass", 120.0);
            h.set_param("width", 0.5);
            h.set_param("balance", -0.4);
        }
        300 => {
            h.set_param("mode", 1.0);
            h.set_param("mono", 1.0);
        }
        _ => {}
    });
    assert_eq!(
        [defaults, width, split, rotation, modes],
        [
            0x3fb4_3677_e7a5_9101,
            0x8f2c_dbdb_62c9_43e2,
            0x7a55_cd14_3bbc_a144,
            0x812c_01ee_6ae4_bb24,
            0x90ca_9bed_b584_3767
        ],
        "{defaults:#018x} {width:#018x} {split:#018x} {rotation:#018x} {modes:#018x}"
    );
}
