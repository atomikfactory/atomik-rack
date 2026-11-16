//! Stereo tool tests, all through `DeviceHarness`.
#![allow(clippy::float_cmp, clippy::needless_range_loop)]

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
            "lo_width",
            "solo",
            "lo_width_cv_trim",
            "hi_width_cv_trim",
            "xover_cv_trim",
        ],
        &[
            "in_l",
            "in_r",
            "out_l",
            "out_r",
            "lo_width_cv",
            "hi_width_cv",
            "xover_cv",
            "lo_out_l",
            "lo_out_r",
            "hi_out_l",
            "hi_out_r",
        ],
    );
    // Ids of the first release never move; the new ones follow `bypass`.
    for (i, p) in DESCRIPTOR.params.iter().enumerate() {
        assert_eq!(p.id.0 as usize, i, "{}", p.key);
    }
    assert_eq!(DESCRIPTOR.params[3].key, "width");
    assert_eq!(DESCRIPTOR.params[4].key, "low_bypass");
    assert_eq!(DESCRIPTOR.params[9].key, "bypass");
    for (i, k) in ["in_l", "in_r", "out_l", "out_r"].iter().enumerate() {
        assert_eq!(DESCRIPTOR.sockets[i].key, *k);
    }
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
//
// Every scenario runs through the frozen pre-upgrade device (`legacy.rs`) and the new device with
// the same input and the same param moves, and the outputs must match bit for bit. Comparing the
// two live (not against pinned hashes) keeps the check true on every OS, whatever the platform
// libm does to `tan` or `sin_cos`.

/// A deterministic tone with no libm: a parabolic sine from a phase fraction.
fn tone_det(n: usize, hz: f64, rate: f64) -> f32 {
    let phase = (n as f64 * hz / rate).fract();
    let x = 2.0 * phase - 1.0;
    (4.0 * x * (1.0 - x.abs())) as f32
}

/// Renders `batches` batches of tone and noise through `device`; `setup` sets params before each
/// batch runs. Returns the bits of every output sample (left then right per batch).
fn render_bits(
    device: Box<dyn Device>,
    rate: f64,
    batches: usize,
    setup: &impl Fn(&mut DeviceHarness, usize),
) -> Vec<u32> {
    let mut h = DeviceHarness::new(device, rate);
    let mut bits = Vec::new();
    for b in 0..batches {
        let mut xl = [0.0f32; BATCH];
        let mut xr = [0.0f32; BATCH];
        for i in 0..BATCH {
            let n = b * BATCH + i;
            // A tone and noise, with the right channel partly correlated with the left.
            let t = tone_det(n, 110.0, rate);
            xl[i] = 0.4 * t + 0.5 * noise(n, 21);
            xr[i] = 0.2 * t + 0.5 * noise(n, 22);
        }
        setup(&mut h, b);
        *h.input_mut("in_l") = xl;
        *h.input_mut("in_r") = xr;
        h.process();
        for key in ["out_l", "out_r"] {
            bits.extend(h.output(key).iter().map(|v| v.to_bits()));
        }
    }
    bits
}

/// Runs one scenario through the old and the new device and asserts identical bits.
fn assert_same_as_legacy(
    name: &str,
    rate: f64,
    batches: usize,
    setup: impl Fn(&mut DeviceHarness, usize),
) {
    let old = render_bits(
        Box::new(super::legacy::LegacyStereoTool::new()),
        rate,
        batches,
        &setup,
    );
    let new = render_bits(Box::new(StereoTool::new()), rate, batches, &setup);
    assert_eq!(old.len(), new.len(), "{name}: output length");
    if let Some(i) = (0..old.len()).find(|&i| old[i] != new[i]) {
        let per_batch = 2 * BATCH;
        panic!(
            "{name}: first differing sample {i} (batch {}, {} channel, sample {}): legacy {:#010x} ({}), new {:#010x} ({})",
            i / per_batch,
            if i % per_batch < BATCH {
                "left"
            } else {
                "right"
            },
            i % BATCH,
            old[i],
            f32::from_bits(old[i]),
            new[i],
            f32::from_bits(new[i]),
        );
    }
}

#[test]
fn existing_projects_sound_bit_identical_after_the_upgrade() {
    // Defaults: the input passes untouched.
    assert_same_as_legacy("defaults", SR, 200, |_, _| {});
    // Width without a split, with changes mid-ramp.
    assert_same_as_legacy("width", SR, 400, |h, b| match b {
        0 => h.set_param("width", 0.0),
        100 => h.set_param("width", 2.0),
        200 => h.set_param("width", 0.7),
        203 => h.set_param("width", 1.4),
        300 => h.set_param("width", 1.0),
        _ => {}
    });
    // Width with the low band kept: several corners, a corner moved mid-run, width back to 1.
    assert_same_as_legacy("split", SR, 500, |h, b| match b {
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
    assert_same_as_legacy("rotation", SR, 400, |h, b| match b {
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
    assert_same_as_legacy("modes", SR, 400, |h, b| match b {
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
}

#[test]
fn more_legacy_behaviour_stays_bit_identical() {
    let prep = |h: &mut DeviceHarness, rate: f64| {
        let sample_rate = atomik_core::SampleRate::new(rate).expect("rate");
        h.device_mut().prepare(&PrepareInfo {
            sample_rate,
            seed: 0,
        });
    };
    // reset() mid-run, with a split, a width and a rotation.
    assert_same_as_legacy("reset", SR, 400, |h, b| match b {
        0 => {
            h.set_param("low_bypass", 150.0);
            h.set_param("width", 0.4);
            h.set_param("rotate", 60.0);
        }
        150 | 300 => h.device_mut().reset(),
        _ => {}
    });
    // prepare() mid-run at the same rate.
    assert_same_as_legacy("reprepare", SR, 400, |h, b| match b {
        0 => {
            h.set_param("low_bypass", 300.0);
            h.set_param("width", 1.6);
        }
        200 => prep(h, SR),
        _ => {}
    });
    // A second sample rate.
    assert_same_as_legacy("rate96", 96_000.0, 400, |h, b| match b {
        0 => {
            h.set_param("low_bypass", 250.0);
            h.set_param("width", 0.2);
        }
        200 => h.set_param("low_bypass", 90.0),
        _ => {}
    });
    assert_same_as_legacy("rate44", 44_100.0, 300, |h, b| {
        if b == 0 {
            // 500 Hz was the top of the old range; the old device clamps anything above it.
            h.set_param("low_bypass", 500.0);
            h.set_param("width", 1.5);
            h.set_param("mono", 1.0);
        }
    });
    // Rotation, split and width together, the corner moved and the width back to 1.
    assert_same_as_legacy("rot_split", SR, 500, |h, b| match b {
        0 => {
            h.set_param("rotate", 120.0);
            h.set_param("rotate_hz", 200.0);
            h.set_param("low_bypass", 400.0);
            h.set_param("width", 0.0);
        }
        200 => h.set_param("low_bypass", 80.0),
        350 => h.set_param("width", 1.7),
        _ => {}
    });
    // A corner between 20 and 20.5 Hz is no split: the same bits as 20.
    assert_same_as_legacy("near20", SR, 300, |h, b| match b {
        0 => {
            h.set_param("low_bypass", 20.3);
            h.set_param("width", 0.5);
        }
        150 => h.set_param("width", 1.5),
        _ => {}
    });
}

// ----- the two bands, solo, meters, CV and band outputs -----

/// White noise of an independent stream per seed (the `noise` hash above is too weakly mixed for
/// two seeds to be uncorrelated).
fn white(n: usize, seed: u64) -> f32 {
    let mut z = (n as u64).wrapping_add(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    ((z >> 40) as f32 / (1u64 << 24) as f32) * 1.6 - 0.8
}

/// Runs the tool for `batches` batches. `cv` is (lo width, hi width, x-over) per sample.
/// Returns the outputs `[out_l, out_r, lo_l, lo_r, hi_l, hi_r]`.
fn run_all(
    h: &mut DeviceHarness,
    l: impl Fn(usize) -> f32,
    r: impl Fn(usize) -> f32,
    cv: impl Fn(usize) -> [f32; 3],
    batches: usize,
) -> [Vec<f32>; 6] {
    let keys = [
        "out_l", "out_r", "lo_out_l", "lo_out_r", "hi_out_l", "hi_out_r",
    ];
    let mut out: [Vec<f32>; 6] = Default::default();
    for b in 0..batches {
        let mut io = [[0.0f32; BATCH]; 5];
        for i in 0..BATCH {
            let n = b * BATCH + i;
            io[0][i] = l(n);
            io[1][i] = r(n);
            let c = cv(n);
            io[2][i] = c[0];
            io[3][i] = c[1];
            io[4][i] = c[2];
        }
        for (k, key) in ["in_l", "in_r", "lo_width_cv", "hi_width_cv", "xover_cv"]
            .iter()
            .enumerate()
        {
            *h.input_mut(key) = io[k];
        }
        h.process();
        for (o, key) in out.iter_mut().zip(keys) {
            o.extend_from_slice(h.output(key));
        }
    }
    out
}

fn no_cv(_: usize) -> [f32; 3] {
    [0.0; 3]
}

fn meters(h: &DeviceHarness) -> [f32; METER_COUNT] {
    let mut m = [0.0f32; 16];
    let n = h.device().meter_values(&mut m);
    assert_eq!(n, METER_COUNT);
    let mut out = [0.0; METER_COUNT];
    out.copy_from_slice(&m[..METER_COUNT]);
    out
}

/// Amplitude of `hz` in `y` over its last second at `SR`.
fn amp_at(y: &[f32], hz: f64) -> f64 {
    let start = y.len() - SR as usize;
    tone_amplitude(&y[start..], hz, start)
}

/// A pure side signal (L = -R) tone, two seconds.
fn side_tone(h: &mut DeviceHarness, hz: f64, cv: impl Fn(usize) -> [f32; 3]) -> [Vec<f32>; 6] {
    let f = sine(hz, 0.5);
    let batches = (SR as usize).div_ceil(BATCH) * 2;
    run_all(h, &f, |n| -f(n), cv, batches)
}

#[test]
fn the_two_bands_have_widths_of_their_own() {
    // Split at 500 Hz. A side-only tone at 80 Hz (low) and at 8 kHz (high).
    for (lo, hi, low_kept, high_kept) in [
        (0.0, 1.0, false, true),
        (1.0, 0.0, true, false),
        (0.0, 0.0, false, false),
        (1.0, 1.0, true, true),
    ] {
        for (hz, kept) in [(80.0, low_kept), (8_000.0, high_kept)] {
            let mut h = tool();
            h.set_param("low_bypass", 500.0);
            h.set_param("lo_width", lo);
            h.set_param("width", hi);
            let o = side_tone(&mut h, hz, no_cv);
            let a = amp_at(&o[0], hz);
            if kept {
                assert!(a > 0.45, "lo {lo} hi {hi} at {hz}: {a}");
            } else {
                assert!(a < 1e-3, "lo {lo} hi {hi} at {hz}: {a}");
            }
        }
    }
    // The low band at width 2 doubles its side signal.
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    h.set_param("lo_width", 2.0);
    let o = side_tone(&mut h, 80.0, no_cv);
    assert!(amp_at(&o[0], 80.0) > 0.9);
}

#[test]
fn without_a_split_the_whole_signal_is_the_high_band() {
    let mut h = tool();
    h.set_param("lo_width", 0.0);
    h.set_param("width", 0.0);
    let o = side_tone(&mut h, 80.0, no_cv);
    assert!(amp_at(&o[0], 80.0) < 1e-4, "width is the high band's");
    let mut h = tool();
    h.set_param("lo_width", 0.0);
    let o = side_tone(&mut h, 80.0, no_cv);
    assert!(amp_at(&o[0], 80.0) > 0.49, "the low width is not in use");
    // The low outputs are silent, the high ones are the signal.
    assert!(o[2].iter().chain(&o[3]).all(|x| *x == 0.0));
    let f = sine(80.0, 0.5);
    for n in 0..o[4].len() {
        assert_eq!(o[4][n], f(n));
        assert_eq!(o[5][n], -f(n));
    }
}

#[test]
fn solo_picks_a_band_for_the_sum() {
    for (solo, low, high) in [(1.0, 0.5, 0.0), (2.0, 0.0, 0.5), (0.0, 0.5, 0.5)] {
        let mut h = tool();
        h.set_param("low_bypass", 500.0);
        h.set_param("solo", solo);
        let lo = side_tone(&mut h, 80.0, no_cv);
        let mut h = tool();
        h.set_param("low_bypass", 500.0);
        h.set_param("solo", solo);
        let hi = side_tone(&mut h, 8_000.0, no_cv);
        let (a, b) = (amp_at(&lo[0], 80.0), amp_at(&hi[0], 8_000.0));
        assert!((a - low).abs() < 0.02, "solo {solo}: low {a}");
        assert!((b - high).abs() < 0.02, "solo {solo}: high {b}");
    }
    // Solo follows the band widths: the soloed band is the widened one.
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    h.set_param("solo", 1.0);
    h.set_param("lo_width", 0.0);
    let o = side_tone(&mut h, 80.0, no_cv);
    assert!(amp_at(&o[0], 80.0) < 1e-3);
}

#[test]
fn solo_without_a_split_is_silence_for_lo_and_everything_for_hi() {
    let mut h = tool();
    h.set_param("solo", 1.0);
    let o = side_tone(&mut h, 1_000.0, no_cv);
    assert!(o[0].iter().skip(BATCH * 20).all(|x| *x == 0.0));
    let mut h = tool();
    h.set_param("solo", 2.0);
    let o = side_tone(&mut h, 1_000.0, no_cv);
    let f = sine(1_000.0, 0.5);
    for n in BATCH * 20..o[0].len() {
        assert_eq!(o[0][n].to_bits(), f(n).to_bits());
    }
}

#[test]
fn switching_solo_does_not_click() {
    for split in [20.0, 500.0] {
        for (from, to) in [(0.0, 1.0), (0.0, 2.0), (1.0, 2.0), (2.0, 0.0), (1.0, 0.0)] {
            let mut h = tool();
            h.set_param("low_bypass", split);
            h.set_param("solo", from);
            let f = sine(300.0, 0.5);
            let mut all = Vec::new();
            for b in 0..200 {
                if b == 100 {
                    h.set_param("solo", to);
                }
                let o = run_all(&mut h, |n| f(n + b * BATCH), |n| f(n + b * BATCH), no_cv, 1);
                all.extend_from_slice(&o[0]);
            }
            // A 300 Hz sine of 0.5 moves at most 0.0033 per sample; allow a few times that.
            let step = all
                .windows(2)
                .skip(BATCH * 10)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max);
            assert!(step < 0.02, "split {split}, solo {from} -> {to}: {step}");
        }
    }
}

#[test]
fn correlation_reads_plus_one_for_mono_minus_one_for_anti_phase_and_about_zero_for_noise() {
    for (name, r_sign, want) in [("mono", 1.0f32, 1.0f32), ("anti", -1.0, -1.0)] {
        let mut h = tool();
        h.set_param("low_bypass", 500.0);
        run_all(
            &mut h,
            |n| white(n, 1),
            |n| r_sign * white(n, 1),
            no_cv,
            1200,
        );
        let m = meters(&h);
        assert!((m[METER_LO_CORR] - want).abs() < 0.01, "{name} lo {m:?}");
        assert!((m[METER_HI_CORR] - want).abs() < 0.01, "{name} hi {m:?}");
    }
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    run_all(&mut h, |n| white(n, 1), |n| white(n, 2), no_cv, 4000);
    let m = meters(&h);
    assert!(m[METER_HI_CORR].abs() < 0.1, "{m:?}");
    assert!(m[METER_LO_CORR].abs() < 0.35, "{m:?}");
    // With no split the whole signal is the high band and the low one reads 0.
    let mut h = tool();
    run_all(&mut h, |n| white(n, 1), |n| white(n, 1), no_cv, 1200);
    let m = meters(&h);
    assert_eq!((m[METER_LO_CORR], m[METER_SPLIT]), (0.0, 0.0));
    assert!((m[METER_HI_CORR] - 1.0).abs() < 0.01, "{m:?}");
    // Silence reads 0.
    let mut h = tool();
    run_all(&mut h, |_| 0.0, |_| 0.0, no_cv, 100);
    assert_eq!(&meters(&h)[..2], &[0.0, 0.0]);
}

#[test]
fn correlation_is_taken_after_the_band_width() {
    // Uncorrelated noise, high band at width 0: the band is mono, correlation +1.
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    h.set_param("width", 0.0);
    run_all(&mut h, |n| white(n, 1), |n| white(n, 2), no_cv, 2000);
    let m = meters(&h);
    assert!((m[METER_HI_CORR] - 1.0).abs() < 0.01, "{m:?}");
}

#[test]
fn the_active_flags_need_signal_and_a_width_other_than_one() {
    let go = |h: &mut DeviceHarness, level: f32| {
        run_all(
            h,
            |n| level * white(n, 1),
            |n| level * white(n, 2),
            no_cv,
            4000,
        );
        meters(h)
    };
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    let m = go(&mut h, 1.0);
    assert_eq!(
        (m[METER_LO_ACTIVE], m[METER_HI_ACTIVE]),
        (0.0, 0.0),
        "{m:?}"
    );
    assert_eq!(m[METER_SPLIT], 1.0);
    h.set_param("width", 1.5);
    let m = go(&mut h, 1.0);
    assert_eq!(
        (m[METER_LO_ACTIVE], m[METER_HI_ACTIVE]),
        (0.0, 1.0),
        "{m:?}"
    );
    h.set_param("lo_width", 0.5);
    let m = go(&mut h, 1.0);
    assert_eq!(
        (m[METER_LO_ACTIVE], m[METER_HI_ACTIVE]),
        (1.0, 1.0),
        "{m:?}"
    );
    // Silent: no signal, no flag.
    let m = go(&mut h, 0.0);
    assert_eq!(
        (m[METER_LO_ACTIVE], m[METER_HI_ACTIVE]),
        (0.0, 0.0),
        "{m:?}"
    );
    // No split: the low band is never active, even with its width off 1.
    let mut h = tool();
    h.set_param("lo_width", 0.5);
    h.set_param("width", 0.5);
    let m = go(&mut h, 1.0);
    assert_eq!(
        (m[METER_LO_ACTIVE], m[METER_HI_ACTIVE]),
        (0.0, 1.0),
        "{m:?}"
    );
}

#[test]
fn the_band_outputs_add_up_to_the_input_when_the_widths_are_one() {
    for hz in [80.0, 500.0, 3_000.0] {
        let mut h = tool();
        h.set_param("low_bypass", 500.0);
        let f = sine(hz, 0.5);
        let batches = (SR as usize).div_ceil(BATCH) * 2;
        let o = run_all(&mut h, &f, |n| 0.5 * f(n), no_cv, batches);
        let sum: Vec<f32> = o[2].iter().zip(&o[4]).map(|(a, b)| a + b).collect();
        // The sum is an all-pass of the input: the same level at every frequency.
        let a = amp_at(&sum, hz);
        assert!((a - 0.5).abs() < 0.01, "{hz}: {a}");
        let b: Vec<f32> = o[3].iter().zip(&o[5]).map(|(a, b)| a + b).collect();
        assert!((amp_at(&b, hz) - 0.25).abs() < 0.01, "{hz}");
        // And the main output is the input itself.
        for n in 0..o[0].len() {
            assert_eq!(o[0][n].to_bits(), f(n).to_bits());
        }
    }
}

#[test]
fn band_outputs_come_before_solo_balance_mode_and_mono() {
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    h.set_param("solo", 1.0);
    h.set_param("balance", 1.0);
    h.set_param("mono", 1.0);
    h.set_param("mode", 1.0);
    let o = side_tone(&mut h, 8_000.0, no_cv);
    // The main output has lost the high band; the high output still carries it.
    assert!(amp_at(&o[0], 8_000.0) < 1e-3);
    assert!(amp_at(&o[4], 8_000.0) > 0.45);
    assert!(amp_at(&o[5], 8_000.0) > 0.45);
}

#[test]
fn the_cv_inputs_move_the_widths_through_their_trims() {
    // Untrimmed (default 0): the CV does nothing.
    let mut h = tool();
    h.set_param("width", 0.0);
    let o = side_tone(&mut h, 1_000.0, |_| [0.9, 0.5, 0.9]);
    assert!(amp_at(&o[0], 1_000.0) < 1e-4);
    // Hi width: 0 + 1 * 0.5 * 2 = 1, the signal is back.
    let mut h = tool();
    h.set_param("width", 0.0);
    h.set_param("hi_width_cv_trim", 1.0);
    let o = side_tone(&mut h, 1_000.0, |_| [0.0, 0.5, 0.0]);
    assert!((amp_at(&o[0], 1_000.0) - 0.5).abs() < 0.01);
    // A negative trim goes the other way: 1 - 0.5 * 2 = 0.
    let mut h = tool();
    h.set_param("hi_width_cv_trim", -1.0);
    let o = side_tone(&mut h, 1_000.0, |_| [0.0, 0.5, 0.0]);
    assert!(amp_at(&o[0], 1_000.0) < 1e-3);
    // Lo width, with a split.
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    h.set_param("lo_width", 0.0);
    h.set_param("lo_width_cv_trim", 1.0);
    let o = side_tone(&mut h, 80.0, |_| [0.5, 0.0, 0.0]);
    assert!(amp_at(&o[0], 80.0) > 0.45);
    // The high width CV does not touch the low band.
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    h.set_param("lo_width", 0.0);
    h.set_param("lo_width_cv_trim", 1.0);
    let o = side_tone(&mut h, 80.0, |_| [0.0, 0.5, 0.0]);
    assert!(amp_at(&o[0], 80.0) < 1e-3);
    // Out of range CV is clamped, a non-finite one counts as 0.
    let mut h = tool();
    h.set_param("width", 0.0);
    h.set_param("hi_width_cv_trim", 1.0);
    let o = side_tone(&mut h, 1_000.0, |_| [0.0, f32::NAN, 0.0]);
    assert!(amp_at(&o[0], 1_000.0) < 1e-4);
    let o = side_tone(&mut h, 1_000.0, |_| [0.0, 9.0, 0.0]);
    assert!(
        (amp_at(&o[0], 1_000.0) - 1.0).abs() < 0.02,
        "width 2 doubles it"
    );
}

#[test]
fn the_crossover_cv_moves_the_corner_and_never_the_split_switch() {
    // Side-only tone at 1 kHz, hi width 0: with the corner at 500 Hz it is high band, gone.
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    h.set_param("width", 0.0);
    let o = side_tone(&mut h, 1_000.0, |_| [0.0, 0.0, 0.5]);
    // (1 kHz is an octave over the corner: the low band keeps 6 % of it.)
    assert!(amp_at(&o[0], 1_000.0) < 0.05, "trim 0 does nothing");
    // +2 octaves (1 * 0.5 * 4): the corner is 2 kHz and 1 kHz is mostly low band, kept.
    h.set_param("xover_cv_trim", 1.0);
    let o = side_tone(&mut h, 1_000.0, |_| [0.0, 0.0, 0.5]);
    let kept = amp_at(&o[0], 1_000.0);
    assert!(kept > 0.4, "{kept}");
    // CV back to 0: the corner is exactly the param again.
    let o = side_tone(&mut h, 1_000.0, no_cv);
    assert!(amp_at(&o[0], 1_000.0) < 0.05);
    // With no split the CV does nothing.
    let mut h = tool();
    h.set_param("width", 0.0);
    h.set_param("xover_cv_trim", 1.0);
    let o = side_tone(&mut h, 1_000.0, |_| [0.0, 0.0, 1.0]);
    assert!(amp_at(&o[0], 1_000.0) < 1e-4);
    // At full negative CV a split stays a split.
    let mut h = tool();
    h.set_param("low_bypass", 100.0);
    h.set_param("xover_cv_trim", 1.0);
    run_all(
        &mut h,
        |n| white(n, 1),
        |n| white(n, 2),
        |_| [0.0, 0.0, -1.0],
        400,
    );
    assert_eq!(meters(&h)[METER_SPLIT], 1.0);
}

#[test]
fn a_moving_crossover_stays_finite_and_bounded() {
    for (lo, hi) in [(0.0, 0.0), (2.0, 2.0), (1.0, 0.3)] {
        let mut h = tool();
        h.set_param("low_bypass", 300.0);
        h.set_param("xover_cv_trim", 1.0);
        h.set_param("lo_width", lo);
        h.set_param("width", hi);
        // A square CV over the whole range every 20 ms, and a fast saw on top.
        let cv = |n: usize| {
            let square = if (n / 960).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            [0.0, 0.0, square * (0.5 + 0.5 * ((n % 97) as f32 / 97.0))]
        };
        let o = run_all(&mut h, |n| white(n, 1), |n| white(n, 2), cv, 3000);
        for ch in &o {
            let peak = ch.iter().fold(0.0f32, |m, x| {
                assert!(x.is_finite());
                m.max(x.abs())
            });
            assert!(peak < 4.0, "lo {lo} hi {hi}: {peak}");
        }
    }
}

#[test]
fn the_split_and_the_correlation_follow_a_sample_rate_change() {
    for rate in [44_100.0f64, 48_000.0, 96_000.0] {
        let f =
            |n: usize, hz: f64| (0.5 * (std::f64::consts::TAU * hz * n as f64 / rate).sin()) as f32;
        // The corner is in Hz, whatever the rate.
        let mut h = DeviceHarness::new(Box::new(StereoTool::new()), rate);
        h.set_param("low_bypass", 500.0);
        h.set_param("width", 0.0);
        let batches = (rate as usize).div_ceil(BATCH) * 2;
        let o = run_all(
            &mut h,
            |n| f(n, 80.0) + f(n, 8_000.0),
            |n| -f(n, 80.0) - f(n, 8_000.0),
            no_cv,
            batches,
        );
        let start = o[0].len() - rate as usize;
        let tone = |hz: f64| {
            let (mut s, mut c) = (0.0f64, 0.0f64);
            for (i, v) in o[0][start..].iter().enumerate() {
                let ph = std::f64::consts::TAU * hz * (start + i) as f64 / rate;
                s += f64::from(*v) * ph.sin();
                c += f64::from(*v) * ph.cos();
            }
            2.0 * s.hypot(c) / rate
        };
        assert!(tone(80.0) > 0.45, "{rate}: {}", tone(80.0));
        assert!(tone(8_000.0) < 2e-3, "{rate}: {}", tone(8_000.0));
        // Correlation: about a 300 ms time constant. After one constant of anti-phase
        // following a long mono stretch the reading is 2/e - 1 = -0.264.
        let mut h = DeviceHarness::new(Box::new(StereoTool::new()), rate);
        let t1k = |n: usize| f(n, 1_000.0);
        run_all(&mut h, t1k, t1k, no_cv, (rate as usize).div_ceil(BATCH) * 2);
        assert!((meters(&h)[METER_HI_CORR] - 1.0).abs() < 0.01);
        let tau = (0.3 * rate / BATCH as f64).round() as usize;
        run_all(&mut h, t1k, |n| -t1k(n), no_cv, tau);
        let c = meters(&h)[METER_HI_CORR];
        assert!((c + 0.264).abs() < 0.1, "{rate}: {c}");
    }
}

// ----- review fixes -----

/// Largest step between neighbouring samples of `y`.
fn max_step(y: &[f32]) -> f32 {
    y.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn touching_the_low_width_while_split_does_not_click() {
    // A mono sine on the corner: the band sum is its all-pass (180 degrees there), so a hard
    // switch between the dry input and the sum is a step of up to twice the amplitude.
    let f = |n: usize| 0.5 * (std::f32::consts::TAU * 500.0 * n as f32 / SR as f32).sin();
    let own = 0.5 * std::f32::consts::TAU * 500.0 / SR as f32;
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    let (l, _) = run(&mut h, f, f, 200);
    assert!(max_step(&l[l.len() / 2..]) < 1.2 * own);
    let mut at = 200;
    for width in [0.0, 1.0] {
        h.set_param("lo_width", width);
        let (l, _) = run(&mut h, |n| f(n + at * BATCH), |n| f(n + at * BATCH), 200);
        at += 200;
        let step = max_step(&l);
        assert!(step < 3.0 * own, "lo_width {width}: step {step} vs {own}");
    }
    // Back at rest the output is the input again, bit for bit.
    let (l, _) = run(&mut h, |n| f(n + at * BATCH), |n| f(n + at * BATCH), 20);
    for (i, v) in l.iter().enumerate() {
        assert_eq!(v.to_bits(), f(i + at * BATCH).to_bits());
    }
}

#[test]
fn a_reset_puts_the_crossover_back_on_its_param_after_a_cv() {
    let mut h = tool();
    h.set_param("low_bypass", 500.0);
    h.set_param("width", 0.0);
    h.set_param("xover_cv_trim", 1.0);
    // +2 octaves: the corner is 2 kHz and the 1 kHz side tone is kept.
    let o = side_tone(&mut h, 1_000.0, |_| [0.0, 0.0, 0.5]);
    assert!(amp_at(&o[0], 1_000.0) > 0.4);
    // What Bypassable::stop does when the effect is bypassed.
    h.device_mut().reset();
    let o = side_tone(&mut h, 1_000.0, no_cv);
    assert!(amp_at(&o[0], 1_000.0) < 0.05, "{}", amp_at(&o[0], 1_000.0));
}

#[test]
fn entering_the_split_starts_from_clean_filters() {
    let mut h = tool();
    h.set_param("lo_width", 0.0);
    h.set_param("width", 0.0);
    h.set_param("low_bypass", 200.0);
    run(&mut h, |n| white(n, 3), |n| white(n, 4), 100);
    // No split for a while, then silence into a split again.
    h.set_param("low_bypass", 20.0);
    run(&mut h, |_| 0.0, |_| 0.0, 10);
    h.set_param("low_bypass", 200.0);
    let o = run_all(&mut h, |_| 0.0, |_| 0.0, no_cv, 20);
    for v in o.iter().flatten() {
        assert_eq!(*v, 0.0);
    }
}

#[test]
fn the_correlation_state_never_goes_subnormal_with_one_channel_silent() {
    let mut c = Corr::default();
    let ca = exp2_det_f64(-std::f64::consts::LOG2_E / (0.3 * 48_000.0));
    for _ in 0..48_000 {
        c.push(ca, 0.3, 0.3);
    }
    for n in 0..12_000_000usize {
        c.push(ca, 0.3, 0.0);
        if n % BATCH == BATCH - 1 {
            c.flush();
            for v in [c.ll, c.rr, c.lr] {
                assert!(v == 0.0 || v.is_normal(), "{v:e} at {n}");
            }
        }
    }
    assert_eq!(c.rr, 0.0);
    assert!(c.ll > 0.0);
}
