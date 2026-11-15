//! Gain tool tests, all through `DeviceHarness` (which guards every `process` against allocation).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max};
use atomik_testkit::harness::DeviceHarness;

fn tool() -> DeviceHarness {
    harness(Box::new(GainTool::new()))
}

/// A deterministic test signal.
fn noise(n: usize, seed: u32) -> f32 {
    let mut x = (n as u32).wrapping_mul(2_654_435_761).wrapping_add(seed);
    x ^= x >> 15;
    x = x.wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    (x as f32 / u32::MAX as f32) * 1.6 - 0.8
}

/// Runs `batches` batches of `l(n)` / `r(n)` on `in_l` / `in_r` and returns the outputs.
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
            "gain",
            "pan",
            "width",
            "blend",
            "mute",
            "gain_cv_trim",
            "pan_cv_trim",
            "bypass",
            "fade",
            "invert_l",
            "invert_r",
            "mono",
            "pan_l",
            "pan_r",
            "pan_law",
            "fader_mode",
            "peak_reset",
            "peak_auto",
        ],
        &[
            "in_l",
            "in_r",
            "aux_l",
            "aux_r",
            "out_l",
            "out_r",
            "gain_cv_in",
            "pan_cv_in",
            "mute_cv_in",
        ],
    );
    // Ids of the params and sockets that shipped first never move.
    for (id, key) in [
        (0, "mode"),
        (1, "gain"),
        (5, "mute"),
        (7, "pan_cv_trim"),
        (8, "bypass"),
    ] {
        assert_eq!(DESCRIPTOR.params[id].key, key);
        assert_eq!(DESCRIPTOR.params[id].id.0 as usize, id);
    }
    for (id, key) in [
        (0, "in_l"),
        (5, "out_r"),
        (6, "gain_cv_in"),
        (7, "pan_cv_in"),
    ] {
        assert_eq!(DESCRIPTOR.sockets[id].key, key);
    }
    // The new ones default to the old sound.
    let d = |k: &str| DESCRIPTOR.param_by_key(k).unwrap().default;
    assert_eq!(
        [
            d("fade"),
            d("invert_l"),
            d("invert_r"),
            d("mono"),
            d("pan_l"),
            d("pan_r")
        ],
        [0.0, 0.0, 0.0, 0.0, -1.0, 1.0]
    );
    assert_eq!(
        (d("fader_mode"), d("peak_reset"), d("peak_auto")),
        (0.0, 0.0, 0.0)
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.gain-tool");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn defaults_pass_the_input_bit_for_bit() {
    let mut h = tool();
    let (l, r) = run(&mut h, |n| noise(n, 1), |n| noise(n, 2), 40);
    for n in 0..l.len() {
        assert_eq!(l[n].to_bits(), noise(n, 1).to_bits());
        assert_eq!(r[n].to_bits(), noise(n, 2).to_bits());
    }
}

#[test]
fn gain_scales_by_the_decibels_and_settles() {
    let mut h = tool();
    h.set_param("gain", 6.020_6);
    let (l, r) = run(&mut h, |_| 0.25, |_| -0.1, 80);
    assert!((last(&l) - 0.5).abs() < 1e-4, "{}", last(&l));
    assert!((last(&r) + 0.2).abs() < 1e-4, "{}", last(&r));
    h.set_param("gain", -20.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.0, 200);
    assert!((last(&l) - 0.05).abs() < 1e-5, "{}", last(&l));
}

#[test]
fn mute_ramps_to_silence_and_back() {
    let mut h = tool();
    run(&mut h, |_| 0.5, |_| 0.5, 4);
    h.set_param("mute", 1.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.0);
    // The first batch after the switch does not jump to silence.
    assert!(l[0] > 0.3, "{}", l[0]);
    h.set_param("mute", 0.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.5);
}

#[test]
fn balance_silences_the_far_side() {
    let mut h = tool();
    h.set_param("pan", 1.0);
    let (l, r) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.0);
    assert_eq!(last(&r), 0.5);
    h.set_param("pan", -0.5);
    let (l, r) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.5);
    assert!((last(&r) - 0.25).abs() < 1e-6);
}

#[test]
fn width_zero_is_mono_and_two_doubles_the_side() {
    let mut h = tool();
    h.set_param("width", 0.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    assert!((last(&l) - 0.5).abs() < 1e-6 && (last(&r) - 0.5).abs() < 1e-6);
    h.set_param("width", 2.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    // M = 0.5, S = 0.3 -> 0.5 +/- 0.6.
    assert!((last(&l) - 1.1).abs() < 1e-6 && (last(&r) + 0.1).abs() < 1e-6);
}

#[test]
fn mix_adds_the_aux_and_xfade_blends_to_it() {
    let mut h = tool();
    h.set_param("mode", 1.0);
    h.set_param("blend", 0.5);
    *h.input_mut("aux_l") = [0.4; BATCH];
    *h.input_mut("aux_r") = [-0.4; BATCH];
    let (l, r) = run(&mut h, |_| 0.2, |_| 0.2, 120);
    assert!((last(&l) - 0.4).abs() < 1e-6 && (last(&r) - 0.0).abs() < 1e-6);

    let mut h = tool();
    h.set_param("mode", 2.0);
    h.set_param("blend", 0.25);
    *h.input_mut("aux_l") = [1.0; BATCH];
    *h.input_mut("aux_r") = [1.0; BATCH];
    let (l, _) = run(&mut h, |_| 0.0, |_| 0.0, 120);
    assert!((last(&l) - 0.25).abs() < 1e-6);
    // Blend 0 is the input alone, bit for bit.
    h.set_param("blend", 0.0);
    let (l, _) = run(&mut h, |n| noise(n, 3), |_| 0.0, 120);
    assert_eq!(last(&l).to_bits(), noise(120 * BATCH - 1, 3).to_bits());
}

#[test]
fn cv_moves_the_gain_by_twenty_four_decibels_at_full_trim() {
    let mut h = tool();
    *h.input_mut("gain_cv_in") = [0.5; BATCH];
    let (l, _) = run(&mut h, |_| 0.1, |_| 0.1, 120);
    // +12 dB.
    assert!((last(&l) - 0.1 * 3.981_07).abs() < 1e-3, "{}", last(&l));
    h.set_param("gain_cv_trim", 0.0);
    let (l, _) = run(&mut h, |_| 0.1, |_| 0.1, 120);
    assert!((last(&l) - 0.1).abs() < 1e-6);
    // The balance CV: +1 at full trim is hard right.
    h.set_param("pan_cv_trim", 1.0);
    *h.input_mut("pan_cv_in") = [1.0; BATCH];
    let (l, r) = run(&mut h, |_| 0.3, |_| 0.3, 120);
    assert_eq!(last(&l), 0.0);
    assert!((last(&r) - 0.3).abs() < 1e-6);
}

#[test]
fn non_finite_input_is_treated_as_silence() {
    let mut h = tool();
    h.set_param("gain", 12.0);
    let (l, r) = run(&mut h, |_| f32::NAN, |_| f32::INFINITY, 4);
    assert!(l.iter().chain(&r).all(|x| *x == 0.0));
}

#[test]
fn the_registered_device_has_a_bypass_that_passes_the_input() {
    let f = factory();
    let mut h = DeviceHarness::new((f.create)(), SR);
    h.set_param("gain", 12.0);
    h.set_param("bypass", 1.0);
    let (l, _) = run(&mut h, |n| noise(n, 5), |_| 0.0, 40);
    assert_eq!(last(&l).to_bits(), noise(40 * BATCH - 1, 5).to_bits());
}

#[test]
fn params_at_their_limits_keep_the_output_finite() {
    let mut h = tool();
    *h.input_mut("in_l") = [0.9; BATCH];
    *h.input_mut("in_r") = [-0.9; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

// ----- regression guard: projects saved before the upgrade must sound bit-identical -----

fn fnv(h: &mut u64, v: f32) {
    for b in v.to_bits().to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Renders `batches` batches; `setup` sets params and the inputs of each batch before it runs.
/// Returns the FNV-1a hash of every output sample (left then right per batch).
#[allow(clippy::needless_range_loop)]
fn render_hash(
    batches: usize,
    mut setup: impl FnMut(&mut DeviceHarness, usize, &mut [[f32; BATCH]; 6]),
) -> u64 {
    let mut h = tool();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in 0..batches {
        // in_l, in_r, aux_l, aux_r, gain cv, pan cv
        let mut io = [[0.0f32; BATCH]; 6];
        for i in 0..BATCH {
            let n = b * BATCH + i;
            io[0][i] = noise(n, 11);
            io[1][i] = noise(n, 12);
            io[2][i] = noise(n, 13) * 0.5;
            io[3][i] = noise(n, 14) * 0.5;
        }
        setup(&mut h, b, &mut io);
        for (k, key) in ["in_l", "in_r", "aux_l", "aux_r", "gain_cv_in", "pan_cv_in"]
            .iter()
            .enumerate()
        {
            *h.input_mut(key) = io[k];
        }
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
#[allow(clippy::needless_range_loop)]
fn existing_projects_sound_bit_identical_after_the_upgrade() {
    // Gain: -60, +24, in-between, with mid-ramp changes.
    let gain = render_hash(600, |h, b, _| match b {
        0 => h.set_param("gain", -60.0),
        100 => h.set_param("gain", 24.0),
        230 => h.set_param("gain", -7.3),
        233 => h.set_param("gain", 3.1),
        _ => {}
    });
    // Balance and width.
    let pan_width = render_hash(600, |h, b, _| match b {
        0 => h.set_param("pan", -1.0),
        100 => h.set_param("pan", 0.3),
        200 => h.set_param("pan", 1.0),
        300 => {
            h.set_param("pan", 0.0);
            h.set_param("width", 0.0);
        }
        400 => h.set_param("width", 2.0),
        500 => h.set_param("width", 0.7),
        _ => {}
    });
    // Mix then xfade with the aux input.
    let aux = render_hash(500, |h, b, _| match b {
        0 => {
            h.set_param("mode", 1.0);
            h.set_param("blend", 0.3);
        }
        120 => h.set_param("blend", 1.0),
        200 => {
            h.set_param("mode", 2.0);
            h.set_param("blend", 0.8);
        }
        300 => h.set_param("blend", 0.0),
        400 => h.set_param("blend", 0.45),
        _ => {}
    });
    // Mute toggling mid-run, also before a ramp has settled.
    let mute = render_hash(500, |h, b, _| match b {
        10 | 130 | 133 | 300 => h.set_param("mute", 1.0),
        60 | 131 | 136 | 400 => h.set_param("mute", 0.0),
        _ => {}
    });
    // Gain and balance CV, with other trims.
    let cv = render_hash(500, |h, b, io| {
        match b {
            0 => h.set_param("pan_cv_trim", 1.0),
            200 => {
                h.set_param("gain_cv_trim", 0.5);
                h.set_param("pan_cv_trim", -0.5);
            }
            _ => {}
        }
        for i in 0..BATCH {
            let t = (b * BATCH + i) as f32;
            io[4][i] = (t * 0.0031).sin();
            io[5][i] = (t * 0.0017).cos() * 0.9;
        }
    });
    assert_eq!(
        [gain, pan_width, aux, mute, cv],
        [
            0xb96f_c873_de7f_41a0,
            0xe2b4_126d_f257_9897,
            0xf1c7_d588_1f6c_abc6,
            0x18b9_d3fa_1012_1779,
            0x8ad6_7e7d_733b_6a04
        ],
        "{gain:#018x} {pan_width:#018x} {aux:#018x} {mute:#018x} {cv:#018x}"
    );
}

// ----- the upgrade: mute fade, polarity, mono, dual pan, VCA, meters -----

fn meters(h: &DeviceHarness) -> [f32; METER_COUNT] {
    let mut m = [0.0f32; 16];
    let n = h.device().meter_values(&mut m);
    assert!(n >= METER_COUNT);
    let mut out = [0.0; METER_COUNT];
    out.copy_from_slice(&m[..METER_COUNT]);
    out
}

fn batches_for(seconds: f32) -> usize {
    (seconds * SR as f32 / BATCH as f32).round() as usize
}

#[test]
fn a_mute_fade_reaches_silence_after_its_time_and_returns_to_full() {
    let mut h = tool();
    h.set_param("fade", 2000.0);
    run(&mut h, |_| 0.5, |_| 0.5, 20);
    h.set_param("mute", 1.0);
    let n = batches_for(2.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, n + 2);
    // Equal steps in dB: half way through it is at -45 dB.
    let mid = l[n / 2 * BATCH];
    assert!(
        (mid - 0.5 * 10f32.powf(-45.0 / 20.0)).abs() < 0.5 * 0.002,
        "{mid}"
    );
    // Still audible a couple of batches before the end, exact silence at it.
    assert!(l[(n - 3) * BATCH] > 0.0);
    assert_eq!(meters(&h)[METER_FADING], 0.0);
    assert!(l[(n + 1) * BATCH..].iter().all(|x| *x == 0.0));
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 3);
    assert!(l.iter().all(|x| *x == 0.0));
    h.set_param("mute", 0.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, n + 2);
    assert!(l[BATCH] < 1e-3, "starts from silence: {}", l[BATCH]);
    assert!(l[(n + 1) * BATCH..].iter().all(|x| *x == 0.5));
    // Reversing half way takes only as long as it has gone.
    h.set_param("mute", 1.0);
    run(&mut h, |_| 0.5, |_| 0.5, n / 2);
    h.set_param("mute", 0.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, n / 2 + 3);
    assert_eq!(last(&l), 0.5);
}

#[test]
fn the_fading_flag_is_up_only_while_a_fade_runs() {
    let mut h = tool();
    h.set_param("fade", 1000.0);
    run(&mut h, |_| 0.5, |_| 0.5, 10);
    assert_eq!(meters(&h)[METER_FADING], 0.0);
    h.set_param("mute", 1.0);
    run(&mut h, |_| 0.5, |_| 0.5, 100);
    assert_eq!(meters(&h)[METER_FADING], 1.0);
    run(&mut h, |_| 0.5, |_| 0.5, batches_for(1.0));
    assert_eq!(meters(&h)[METER_FADING], 0.0);
}

#[test]
fn the_mute_cv_gate_mutes_and_is_ored_with_the_param() {
    let mut h = tool();
    run(&mut h, |_| 0.5, |_| 0.5, 4);
    *h.input_mut("mute_cv_in") = [1.0; BATCH];
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.0);
    assert!(l[0] > 0.3, "ramps, no jump");
    *h.input_mut("mute_cv_in") = [0.0; BATCH];
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.5);
    h.set_param("mute", 1.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.0);
    *h.input_mut("mute_cv_in") = [1.0; BATCH];
    h.set_param("mute", 0.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.0);
    // With a fade the CV fades too.
    let mut h = tool();
    h.set_param("fade", 500.0);
    run(&mut h, |_| 0.5, |_| 0.5, 4);
    *h.input_mut("mute_cv_in") = [0.9; BATCH];
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, batches_for(0.5) + 3);
    assert_eq!(last(&l), 0.0);
    assert!(l[5 * BATCH] > 0.1);
}

#[test]
fn polarity_flips_each_side_before_everything_else() {
    let mut h = tool();
    h.set_param("invert_l", 1.0);
    let (l, r) = run(&mut h, |n| noise(n, 1), |n| noise(n, 2), 80);
    let k = 80 * BATCH - 1;
    assert_eq!(l[k], -noise(k, 1));
    assert_eq!(r[k], noise(k, 2));
    h.set_param("invert_l", 0.0);
    h.set_param("invert_r", 1.0);
    let (l, r) = run(&mut h, |n| noise(n, 1), |n| noise(n, 2), 400);
    let k = 400 * BATCH - 1;
    assert_eq!(l[k], noise(k, 1));
    assert_eq!(r[k], -noise(k, 2));
    // Before the aux mix: the aux is not flipped.
    h.set_param("mode", 1.0);
    h.set_param("blend", 1.0);
    h.set_param("invert_l", 1.0);
    h.set_param("invert_r", 0.0);
    *h.input_mut("aux_l") = [0.25; BATCH];
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.0, 80);
    assert!((last(&l) + 0.25).abs() < 1e-6, "{}", last(&l));
}

#[test]
fn mono_sums_both_sides_after_polarity() {
    let mut h = tool();
    h.set_param("mono", 1.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    assert_eq!((last(&l), last(&r)), (0.5, 0.5));
    h.set_param("invert_r", 1.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    assert!((last(&l) - 0.3).abs() < 1e-6 && (last(&r) - 0.3).abs() < 1e-6);
    h.set_param("mono", 0.0);
    h.set_param("invert_r", 0.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    assert_eq!((last(&l), last(&r)), (0.8, 0.2));
}

#[test]
fn dual_pan_places_each_input_with_the_pan_law() {
    let centre = [1.0, std::f32::consts::FRAC_1_SQRT_2, 0.594_603_6, 0.5];
    for (law, g) in centre.iter().enumerate() {
        let mut h = tool();
        h.set_param("pan_law", law as f32);
        h.set_param("pan_l", 0.0);
        h.set_param("pan_r", 0.0);
        let (l, r) = run(&mut h, |_| 0.4, |_| 0.0, 200);
        assert!((last(&l) - 0.4 * g).abs() < 1e-5, "law {law}: {}", last(&l));
        assert!((last(&r) - 0.4 * g).abs() < 1e-5, "law {law}: {}", last(&r));
        // Off the centre the nearer side is louder, in every law.
        h.set_param("pan_l", -0.5);
        let (l, r) = run(&mut h, |_| 0.4, |_| 0.0, 200);
        assert!(last(&l) > last(&r) && last(&r) > 0.0, "law {law}");
        // The extremes are unity and silence in every law.
        h.set_param("pan_l", 1.0);
        h.set_param("pan_r", -1.0);
        let (l, r) = run(&mut h, |_| 0.4, |_| 0.2, 200);
        assert_eq!((last(&l), last(&r)), (0.2, 0.4), "law {law}");
    }
    // Both inputs to the left.
    let mut h = tool();
    h.set_param("pan_r", -1.0);
    let (l, r) = run(&mut h, |_| 0.4, |_| 0.2, 200);
    assert!((last(&l) - 0.6).abs() < 1e-6 && last(&r) == 0.0);
}

#[test]
fn the_vca_fader_never_boosts_and_its_bottom_is_silence() {
    let mut h = tool();
    h.set_param("fader_mode", 1.0);
    h.set_param("gain", 6.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 200);
    assert_eq!(last(&l), 0.5);
    // CV cannot lift it past 0 dB either.
    *h.input_mut("gain_cv_in") = [0.5; BATCH];
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 200);
    assert_eq!(last(&l), 0.5);
    *h.input_mut("gain_cv_in") = [0.0; BATCH];
    h.set_param("gain", -6.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 200);
    assert!((last(&l) - 0.5 * 10f32.powf(-0.3)).abs() < 1e-5);
    h.set_param("gain", -59.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 400);
    assert!((last(&l) - 0.5 * 10f32.powf(-59.0 / 20.0)).abs() < 1e-5);
    h.set_param("gain", -60.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 400);
    assert_eq!(last(&l), 0.0);
    // Trim mode: the same value is only -60 dB.
    h.set_param("fader_mode", 0.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 400);
    assert!((last(&l) - 0.0005).abs() < 1e-6, "{}", last(&l));
}

#[test]
fn meters_follow_the_peaks_hold_and_flags() {
    let mut h = tool();
    h.set_param("gain", -6.0206);
    run(&mut h, |_| 0.5, |_| 0.25, 40);
    let m = meters(&h);
    assert_eq!((m[METER_IN_PEAK_L], m[METER_IN_PEAK_R]), (0.5, 0.25));
    assert!((m[METER_OUT_PEAK_L] - 0.25).abs() < 1e-4);
    assert!((m[METER_OUT_PEAK_R] - 0.125).abs() < 1e-4);
    assert!(
        (m[METER_HOLD_DB] + 12.04).abs() < 0.05,
        "{}",
        m[METER_HOLD_DB]
    );
    assert_eq!(m[METER_SIGNAL], 1.0);
    // Peaks fall away, the hold stays.
    run(&mut h, |_| 0.0, |_| 0.0, 40);
    let m = meters(&h);
    assert_eq!((m[METER_IN_PEAK_L], m[METER_OUT_PEAK_R]), (0.0, 0.0));
    assert!((m[METER_HOLD_DB] + 12.04).abs() < 0.05);
    assert_eq!(m[METER_SIGNAL], 0.0);
    // A rising edge clears it, and only the edge.
    h.set_param("peak_reset", 1.0);
    run(&mut h, |_| 0.0, |_| 0.0, 2);
    assert_eq!(meters(&h)[METER_HOLD_DB], -120.0);
    run(&mut h, |_| 0.5, |_| 0.5, 4);
    assert!(
        meters(&h)[METER_HOLD_DB] > -20.0,
        "held at 1 does not keep resetting"
    );
    h.set_param("peak_reset", 0.0);
    run(&mut h, |_| 0.0, |_| 0.0, 4);
    assert!(meters(&h)[METER_HOLD_DB] > -20.0);
    h.set_param("peak_reset", 1.0);
    run(&mut h, |_| 0.0, |_| 0.0, 2);
    assert_eq!(meters(&h)[METER_HOLD_DB], -120.0);
}

#[test]
fn the_hold_resets_itself_every_three_seconds_with_auto_on() {
    let mut h = tool();
    h.set_param("peak_auto", 1.0);
    run(&mut h, |_| 0.5, |_| 0.5, 4);
    run(&mut h, |_| 0.0, |_| 0.0, batches_for(2.5));
    assert!(meters(&h)[METER_HOLD_DB] > -7.0);
    run(&mut h, |_| 0.0, |_| 0.0, batches_for(0.7));
    assert_eq!(meters(&h)[METER_HOLD_DB], -120.0);
    // Without auto it stays.
    let mut h = tool();
    run(&mut h, |_| 0.5, |_| 0.5, 4);
    run(&mut h, |_| 0.0, |_| 0.0, batches_for(4.0));
    assert!(meters(&h)[METER_HOLD_DB] > -7.0);
}

#[test]
fn the_signal_and_cv_flags() {
    let mut h = tool();
    run(&mut h, |_| 0.0004, |_| 0.0, 40);
    assert_eq!(meters(&h)[METER_SIGNAL], 0.0);
    run(&mut h, |_| 0.0, |_| 0.01, 40);
    assert_eq!(meters(&h)[METER_SIGNAL], 1.0);
    assert_eq!(meters(&h)[METER_CV_ACTIVE], 0.0);
    for cv in ["gain_cv_in", "pan_cv_in", "mute_cv_in"] {
        *h.input_mut(cv) = [0.2; BATCH];
        run(&mut h, |_| 0.0, |_| 0.0, 3);
        assert_eq!(meters(&h)[METER_CV_ACTIVE], 1.0, "{cv}");
        *h.input_mut(cv) = [0.0; BATCH];
        run(&mut h, |_| 0.0, |_| 0.0, 40);
        assert_eq!(meters(&h)[METER_CV_ACTIVE], 0.0, "{cv}");
    }
}

/// Largest difference between neighbouring samples.
fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
}

#[test]
fn switching_a_toggle_or_choice_does_not_click() {
    let sine = |n: usize| 0.5 * (n as f32 * 0.0575).sin();
    let sine2 = |n: usize| 0.4 * (n as f32 * 0.0863).sin();
    // (The old `mode` choice is not in the list: it has always switched at once.)
    let setups: [&[(&str, f32)]; 7] = [
        &[("invert_l", 1.0)],
        &[("invert_r", 1.0)],
        &[("mono", 1.0)],
        &[("pan_law", 3.0)],
        &[("pan_law", 0.0)],
        &[("fader_mode", 1.0)],
        &[("mute", 1.0)],
    ];
    for changes in setups {
        let mut h = tool();
        // Off-centre pans and a gain above 0 dB so every switch has something to change.
        h.set_param("pan_l", -0.3);
        h.set_param("pan_r", 0.6);
        h.set_param("gain", 6.0);
        h.set_param("blend", 0.5);
        *h.input_mut("aux_l") = [0.3; BATCH];
        let (mut l, mut r) = run(&mut h, sine, sine2, 60);
        let base = max_step(&l).max(max_step(&r));
        for (k, v) in changes {
            h.set_param(k, *v);
        }
        let (l1, r1) = run(
            &mut h,
            |n| sine(n + 60 * BATCH),
            |n| sine2(n + 60 * BATCH),
            200,
        );
        l.extend(&l1);
        r.extend(&r1);
        let after = max_step(&l).max(max_step(&r));
        // The switch is a short ramp, not a jump.
        assert!(after < 2.0 * base + 0.02, "{changes:?}: {after} vs {base}");
    }
}

#[test]
fn nan_is_safe_with_every_new_stage_on() {
    let mut h = tool();
    for (k, v) in [
        ("invert_l", 1.0),
        ("mono", 1.0),
        ("pan_l", 0.2),
        ("pan_r", -0.3),
        ("fader_mode", 1.0),
        ("fade", 100.0),
        ("mute", 1.0),
    ] {
        h.set_param(k, v);
    }
    *h.input_mut("mute_cv_in") = [f32::NAN; BATCH];
    let (l, r) = run(&mut h, |_| f32::NAN, |_| f32::INFINITY, 20);
    assert!(l.iter().chain(&r).all(|x| *x == 0.0));
    assert!(meters(&h).iter().all(|x| x.is_finite()));
}

#[test]
fn switching_the_fade_on_while_muting_never_swells_back() {
    let mut h = tool();
    run(&mut h, |_| 0.5, |_| 0.5, 20);
    h.set_param("mute", 1.0);
    // A few batches into the smoothed mute, the level is partway down.
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 2);
    let level = last(&l);
    assert!(level > 0.0 && level < 0.5, "{level}");
    h.set_param("fade", 1000.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, batches_for(1.2));
    let mut prev = level;
    for x in &l {
        assert!(*x <= prev + 1e-6, "swelled: {x} after {prev}");
        prev = *x;
    }
    assert_eq!(last(&l), 0.0);
}
