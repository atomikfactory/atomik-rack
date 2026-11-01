//! The per-pad tone, cuts, drive and sends.

#![allow(clippy::float_cmp)]
use super::*;
use atomik_testkit::no_alloc::assert_no_alloc;
use atomik_testkit::signal::{peak, rms};

const FRAMES: usize = 20_000;

/// A kit with noise on pad 3.
fn kit() -> SampleMapRef {
    map(
        vec![buffer(vec![noise(FRAMES, 5), noise(FRAMES, 6)])],
        vec![zone(0, FRAMES as u64, key_of(3))],
    )
}

/// Renders one hit of pad 3 for `batches` batches after `setup`.
fn render_pad3(setup: impl Fn(&mut DeviceHarness), batches: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut h, _) = with_map(&kit());
    setup(&mut h);
    hit(&mut h, 0, 3, 1.0);
    render(&mut h, batches)
}

#[test]
fn pad_effects_at_their_defaults_change_nothing() {
    let base = render_pad3(|_| {}, 40);
    // Moving every effect away and back lands on the same bits as never touching them.
    let back = render_pad3(
        |h| {
            for k in ["tone", "lowcut", "highcut", "drive", "send1", "send2"] {
                h.set_param(&format!("pad3.{k}"), 0.7);
            }
            for (k, v) in [
                ("tone", 0.0),
                ("lowcut", 20.0),
                ("highcut", 20_000.0),
                ("drive", 0.0),
                ("send1", 0.0),
                ("send2", 0.0),
            ] {
                h.set_param(&format!("pad3.{k}"), v);
            }
        },
        40,
    );
    assert_eq!(bits(&base.0), bits(&back.0));
    assert_eq!(bits(&base.1), bits(&back.1));
    assert!(peak(&base.0) > 0.1);
}

#[test]
fn the_cuts_remove_what_is_below_and_above_them() {
    // Energy below about 375 Hz (the means of blocks of 64 samples) and above it (what is left).
    let bands = |lo: f32, hi: f32| {
        let (l, _) = render_pad3(
            |h| {
                h.set_param("pad3.lowcut", lo);
                h.set_param("pad3.highcut", hi);
            },
            120,
        );
        let x = &l[2_000..];
        let lows: Vec<f32> = x
            .as_chunks::<64>()
            .0
            .iter()
            .map(|c| c.iter().sum::<f32>() / 64.0)
            .collect();
        let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
        (rms(&lows), rms(&d))
    };
    let (open_low, open_high) = bands(20.0, 20_000.0);
    let (cut_low, cut_high) = bands(2_000.0, 20_000.0);
    assert!(cut_low < open_low * 0.3, "{cut_low} vs {open_low}");
    assert!(cut_high > open_high * 0.8, "the highs stay");
    let (dark_low, dark_high) = bands(20.0, 500.0);
    assert!(dark_high < open_high * 0.3, "{dark_high} vs {open_high}");
    assert!(dark_low > open_low * 0.5, "the lows stay");
}

#[test]
fn tone_tilts_the_balance_between_low_and_high() {
    // A one-pole split at 1 kHz is enough to compare: the difference signal carries the highs.
    let highs = |tone: f32| {
        let (l, _) = render_pad3(|h| h.set_param("pad3.tone", tone), 120);
        let d: Vec<f32> = l.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&d[2_000..]) / rms(&l[2_000..])
    };
    let (dark, flat, bright) = (highs(-1.0), highs(0.0), highs(1.0));
    assert!(dark < flat && flat < bright, "{dark} {flat} {bright}");
}

#[test]
fn drive_saturates_and_stays_bounded() {
    let (clean, _) = render_pad3(|_| {}, 120);
    let (driven, _) = render_pad3(|h| h.set_param("pad3.drive", 1.0), 120);
    assert!(driven.iter().all(|s| s.is_finite()));
    assert!(
        peak(&driven) <= 1.0,
        "the soft limiter and the shaper bound it"
    );
    assert_ne!(bits(&clean), bits(&driven));
    // A driven noise burst is denser: its crest factor drops.
    let crest = |x: &[f32]| peak(x) / rms(x);
    assert!(crest(&driven[200..4_000]) < crest(&clean[200..4_000]));
}

#[test]
#[allow(clippy::needless_range_loop)]
fn sends_carry_the_pad_at_their_level_and_leave_the_main_mix_alone() {
    let (mut h, _) = with_map(&kit());
    for k in ["send1_l", "send1_r", "send2_l", "send2_r"] {
        h.set_connected(k, true);
    }
    h.set_param("pad3.send1", 0.5);
    h.set_param("pad3.send2", 1.0);
    hit(&mut h, 0, 3, 1.0);
    let outs = h.render(
        40,
        &["out_l", "out_r", "send1_l", "send1_r", "send2_l", "send2_r"],
    );
    // A send does not take away from the main mix: the same pad, with no send, sounds the same.
    let (plain, _) = render_pad3(|_| {}, 40);
    assert_eq!(bits(&outs[0]), bits(&plain));
    for i in 0..outs[0].len() {
        assert!((outs[2][i] - 0.5 * outs[0][i]).abs() < 1e-6, "{i}");
        assert!((outs[4][i] - outs[0][i]).abs() < 1e-6, "{i}");
        assert!((outs[3][i] - 0.5 * outs[1][i]).abs() < 1e-6, "{i}");
    }
    assert!(peak(&outs[2]) > 0.05);
}

#[test]
fn a_routed_pad_sends_its_processed_signal_to_its_own_output() {
    let (mut h, _) = with_map(&kit());
    connect_pad(&mut h, 3);
    h.set_connected("send1_l", true);
    h.set_param("pad3.send1", 1.0);
    h.set_param("pad3.lowcut", 2_000.0);
    hit(&mut h, 0, 3, 1.0);
    let o = h.render(60, &["out_l", "pad3_out_l", "send1_l"]);
    assert_eq!(peak(&o[0]), 0.0, "a routed pad stays out of the main mix");
    assert!(peak(&o[1]) > 0.05);
    assert_eq!(
        bits(&o[1]),
        bits(&o[2]),
        "send 1 at 100 % equals the pad's output"
    );
    let (raw, _) = {
        let (mut h, _) = with_map(&kit());
        connect_pad(&mut h, 3);
        hit(&mut h, 0, 3, 1.0);
        let o = h.render(60, &["pad3_out_l", "pad3_out_r"]);
        (o[0].clone(), o[1].clone())
    };
    assert_ne!(
        bits(&o[1]),
        bits(&raw),
        "the low cut acted on the routed output"
    );
}

#[test]
fn the_effect_chain_does_not_allocate_and_stays_finite_for_any_values() {
    let (mut h, _) = with_map(&kit());
    for p in 1..=PAD_COUNT {
        for (k, v) in [
            ("tone", 1.0),
            ("lowcut", 300.0),
            ("highcut", 4_000.0),
            ("drive", 0.8),
            ("send1", 0.4),
            ("send2", 0.6),
        ] {
            h.set_param(&format!("pad{p}.{k}"), v);
        }
    }
    for k in ["send1_l", "send1_r", "send2_l", "send2_r"] {
        h.set_connected(k, true);
    }
    for p in 1..=PAD_COUNT {
        hit(&mut h, p as u16, p, 1.0);
    }
    h.process();
    assert_no_alloc(|| {
        for _ in 0..30 {
            h.process();
        }
    });
    for k in ["out_l", "send1_l", "send2_r"] {
        assert!(h.output(k).iter().all(|s| s.is_finite()), "{k}");
    }
    // Out-of-range and NaN values are clamped.
    for v in [f32::NAN, -5.0, 1.0e9] {
        for k in ["tone", "lowcut", "highcut", "drive", "send1", "send2"] {
            h.set_param(&format!("pad1.{k}"), v);
        }
        hit(&mut h, 0, 1, 1.0);
        for _ in 0..5 {
            h.process();
            assert!(h.output("out_l").iter().all(|s| s.is_finite()));
        }
    }
}
