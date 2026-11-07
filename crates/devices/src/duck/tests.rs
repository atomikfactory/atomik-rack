//! Sidechain tool tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, on, sweep_params_min_max};
use atomik_dsp::duck_curve::{COUNT_KEY, POINT_KEYS};
use atomik_testkit::harness::DeviceHarness;

fn duck() -> DeviceHarness {
    harness(Box::new(Duck::new()))
}

/// At 120 BPM one quarter note is 24 000 frames at 48 kHz.
const QUARTER: usize = 24_000;

fn ms(t: f64) -> usize {
    (t * 0.001 * SR).round() as usize
}

/// Feeds a constant `level` on the main inputs (and `key` on the sidechain when given) for
/// `samples` samples; returns the left output and the CV out per sample.
fn feed(
    h: &mut DeviceHarness,
    level: f32,
    key: Option<f32>,
    samples: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut out, mut cv) = (Vec::new(), Vec::new());
    for _ in 0..samples.div_ceil(BATCH) {
        *h.input_mut("in_l") = [level; BATCH];
        *h.input_mut("in_r") = [level; BATCH];
        if let Some(k) = key {
            *h.input_mut("sc_in_l") = [k; BATCH];
            *h.input_mut("sc_in_r") = [k; BATCH];
        }
        h.process();
        out.extend_from_slice(h.output("out_l"));
        cv.extend_from_slice(h.output("duck_cv_out"));
    }
    (out, cv)
}

#[test]
fn descriptor_validates() {
    let params: Vec<&str> = [
        "mode",
        "rate",
        "depth",
        "attack",
        "time",
        "curve",
        "threshold",
        "send",
        "length",
        "trigger",
        "band",
        "band_freq",
        "bypass",
        COUNT_KEY,
    ]
    .into_iter()
    .chain(POINT_KEYS.iter().flatten().copied())
    .collect();
    assert_keys(
        &DESCRIPTOR,
        &params,
        &[
            "in_l",
            "in_r",
            "sc_in_l",
            "sc_in_r",
            "out_l",
            "out_r",
            "duck_cv_out",
            "notes_in",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.duck");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn the_curve_falls_over_the_attack_and_recovers_over_the_time() {
    assert_eq!(dip_at(0.0, 2.0, 100.0, 1.0), 0.0);
    assert!((dip_at(1.0, 2.0, 100.0, 1.0) - 0.5).abs() < 1e-6);
    assert!((dip_at(2.0, 2.0, 100.0, 1.0) - 1.0).abs() < 1e-5);
    assert!((dip_at(52.0, 2.0, 100.0, 1.0) - 0.5).abs() < 1e-5);
    assert_eq!(dip_at(102.0, 2.0, 100.0, 1.0), 0.0);
    // A curve above 1 hangs low, below 1 comes back early.
    assert!((dip_at(52.0, 2.0, 100.0, 2.0) - 0.25).abs() < 1e-5);
    assert!((dip_at(52.0, 2.0, 100.0, 0.5) - 0.5f32.sqrt()).abs() < 1e-5);
    assert_eq!(dip_at(-3.0, 2.0, 100.0, 1.0), 0.0);
}

#[test]
fn pump_ducks_by_the_depth_on_the_beat_and_recovers_in_the_time() {
    let mut h = duck();
    h.set_param("depth", 0.8);
    h.set_param("attack", 2.0);
    h.set_param("time", 250.0);
    h.set_param("rate", 1.0);
    let (out, cv) = feed(&mut h, 0.5, None, QUARTER * 2);
    // On the beat the gain is still 1; after the attack it is 1 - depth.
    assert_eq!(out[0], 0.5);
    assert!((out[ms(2.0)] - 0.5 * 0.2).abs() < 1e-4, "{}", out[ms(2.0)]);
    // Halfway through the recovery (2 + 125 ms) the dip is 0.5.
    let mid = out[ms(127.0)];
    assert!((mid - 0.5 * (1.0 - 0.4)).abs() < 1e-3, "{mid}");
    assert!((cv[ms(127.0)] - 0.4).abs() < 1e-3);
    // Recovered after the time, until the next beat.
    assert_eq!(out[ms(260.0)], 0.5);
    assert_eq!(cv[ms(260.0)], 0.0);
    // The next beat does it again.
    assert!((out[QUARTER + ms(2.0)] - 0.1).abs() < 1e-4);
}

#[test]
fn pump_rate_sets_the_cycle() {
    let mut h = duck();
    h.set_param("rate", 2.0); // 1/8: 12 000 frames
    h.set_param("time", 100.0);
    let (out, _) = feed(&mut h, 0.5, None, QUARTER);
    assert!((out[ms(2.0)] - 0.1).abs() < 1e-4);
    assert!((out[12_000 + ms(2.0)] - 0.1).abs() < 1e-4);
    assert_eq!(out[12_000 - 1], 0.5);
}

#[test]
fn pump_is_the_same_after_a_locate_as_playing_through() {
    let mut a = duck();
    let (through, _) = feed(&mut a, 0.5, None, QUARTER * 3);
    // A second device started at a later batch (the gain is a function of the position alone, so
    // there is no history to catch up on) gives the same gain from there on. The harness adds up
    // the tick per batch, so the positions agree to rounding, not to the bit.
    let start_batch = (QUARTER * 2 + 700) / BATCH;
    let start = start_batch * BATCH;
    let mut b = duck();
    let tpf = b.time_mut().ticks_per_frame;
    b.time_mut().frame = start as u64;
    b.time_mut().tick = start as f64 * tpf;
    let (located, _) = feed(&mut b, 0.5, None, QUARTER * 3 - start);
    for (i, y) in located.iter().enumerate() {
        assert!((y - through[start + i]).abs() < 1e-6, "sample {i}");
    }
    // The same position gives the same bits, whatever came before.
    let mut c = duck();
    let mut d = duck();
    for h in [&mut c, &mut d] {
        h.time_mut().tick = 20_000.0;
    }
    let (x, _) = feed(&mut c, 0.5, None, BATCH * 4);
    let (y, _) = feed(&mut d, 0.5, None, BATCH * 4);
    assert!(x.iter().zip(&y).all(|(p, q)| p.to_bits() == q.to_bits()));
    assert!(x.iter().any(|v| *v < 0.5));
}

#[test]
fn pump_is_open_while_the_transport_is_stopped() {
    let mut h = duck();
    h.time_mut().playing = false;
    let (out, cv) = feed(&mut h, 0.5, None, QUARTER);
    assert!(out.iter().all(|x| *x == 0.5));
    assert!(cv.iter().all(|x| *x == 0.0));
}

#[test]
fn pump_follows_the_loop_wrap() {
    let mut h = duck();
    let tpf = h.time_mut().ticks_per_frame;
    // The batch starts 10 frames before the end of a cycle; the loop jumps back to tick 0 at
    // frame 10, which is the start of a cycle (still open), after which the dip falls.
    h.time_mut().tick = 15_360.0 - 10.0 * tpf;
    h.time_mut().loop_wrap = Some(atomik_core::LoopWrap {
        offset: 10,
        to_tick: 0.0,
    });
    let (out, _) = feed(&mut h, 0.5, None, BATCH);
    assert_eq!(out[9], 0.5);
    assert_eq!(out[10], 0.5);
    assert!(out[BATCH - 1] < 0.5);
}

#[test]
fn trigger_starts_the_curve_on_each_hit_and_retriggers_without_a_jump() {
    let mut h = duck();
    h.set_param("mode", 1.0);
    h.set_param("depth", 1.0);
    h.set_param("attack", 2.0);
    h.set_param("time", 100.0);
    // Quiet key: nothing.
    let (out, _) = feed(&mut h, 0.5, Some(0.0), ms(50.0));
    assert!(out.iter().all(|x| *x == 0.5));
    // A hit (one batch), then quiet.
    feed(&mut h, 0.5, Some(0.5), BATCH);
    let (out, cv) = feed(&mut h, 0.5, Some(0.0), ms(200.0));
    assert!(
        cv.iter().cloned().fold(0.0, f32::max) > 0.95,
        "ducked deeply"
    );
    assert_eq!(out[out.len() - 1], 0.5, "recovered");
    // Hit again partway through a recovery: the dip does not jump back to 0.
    feed(&mut h, 0.5, Some(0.5), BATCH);
    let (_, before) = feed(&mut h, 0.5, Some(0.0), ms(40.0));
    let mid = before[before.len() - 1];
    assert!(mid > 0.2 && mid < 0.8, "{mid}");
    let (_, after) = feed(&mut h, 0.5, Some(0.5), BATCH);
    assert!(
        (after[0] - mid).abs() < 0.1,
        "continues from {mid}, not {}",
        after[0]
    );
}

#[test]
fn trigger_without_a_sidechain_listens_to_the_main_input() {
    let mut h = duck();
    h.set_param("mode", 1.0);
    h.set_param("depth", 1.0);
    let (_, cv) = feed(&mut h, 0.5, None, ms(20.0));
    assert!(cv.iter().cloned().fold(0.0, f32::max) > 0.9);
}

#[test]
fn classic_ducks_while_the_key_is_loud_and_recovers_when_it_stops() {
    let mut h = duck();
    h.set_param("mode", 2.0);
    h.set_param("depth", 0.75);
    h.set_param("attack", 1.0);
    h.set_param("time", 50.0);
    h.set_param("threshold", -30.0);
    // A key 12 dB over the threshold ducks the full depth.
    let (out, cv) = feed(&mut h, 0.4, Some(10f32.powf(-18.0 / 20.0)), ms(300.0));
    let last = cv[cv.len() - 1];
    assert!((last - 0.75).abs() < 0.01, "{last}");
    assert!((out[out.len() - 1] - 0.4 * 0.25).abs() < 0.005);
    // A key at the threshold ducks nothing.
    let (_, cv) = feed(&mut h, 0.4, Some(10f32.powf(-30.0 / 20.0)), ms(500.0));
    assert!(cv[cv.len() - 1] < 0.01);
    // Silence: recovered.
    let (out, _) = feed(&mut h, 0.4, Some(0.0), ms(1_000.0));
    assert_eq!(out[out.len() - 1], 0.4);
}

#[test]
fn send_leaves_the_audio_alone_and_moves_the_cv() {
    let mut h = duck();
    h.set_param("send", 1.0);
    h.set_param("depth", 1.0);
    let (out, cv) = feed(&mut h, 0.5, None, ms(10.0));
    assert!(out.iter().all(|x| *x == 0.5));
    assert!(cv.iter().cloned().fold(0.0, f32::max) > 0.9);
}

#[test]
fn depth_zero_is_a_null() {
    let mut h = duck();
    h.set_param("depth", 0.0);
    let (out, cv) = feed(&mut h, 0.37, None, QUARTER);
    assert!(out.iter().all(|x| x.to_bits() == 0.37f32.to_bits()));
    assert!(cv.iter().all(|x| *x == 0.0));
}

#[test]
fn the_registered_device_bypasses_to_the_input() {
    let mut h = DeviceHarness::new((factory().create)(), SR);
    h.set_param("bypass", 1.0);
    let (out, _) = feed(&mut h, 0.5, None, ms(300.0));
    assert!(out.iter().all(|x| *x == 0.5));
}

#[test]
fn params_at_their_limits_keep_the_output_finite() {
    for mode in 0..4 {
        let mut h = duck();
        h.set_param("mode", mode as f32);
        *h.input_mut("in_l") = [0.9; BATCH];
        *h.input_mut("in_r") = [-0.3; BATCH];
        *h.input_mut("sc_in_l") = [0.7; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }
}

// ------------------------------------------------------------------ the drawn curve

/// Draws `points` into the hidden params of the device.
fn draw(h: &mut DeviceHarness, points: &[(f32, f32, i8)]) {
    h.set_param(COUNT_KEY, points.len() as f32);
    for (i, (x, y, bend)) in points.iter().enumerate() {
        h.set_param(POINT_KEYS[i][0], *x);
        h.set_param(POINT_KEYS[i][1], *y);
        h.set_param(POINT_KEYS[i][2], f32::from(*bend));
    }
}

/// A device in curve mode with a ramp up over the first tenth and a straight way back.
fn curve_duck() -> DeviceHarness {
    let mut h = duck();
    h.set_param("mode", 3.0);
    h.set_param("depth", 1.0);
    h.set_param("length", 2.0); // a quarter note: 24 000 frames at 120 BPM
    draw(&mut h, &[(0.0, 0.0, 0), (0.1, 1.0, 0), (1.0, 0.0, 0)]);
    h
}

/// A key hit: one batch of loud key, with the main input at `level`.
fn hit(h: &mut DeviceHarness, level: f32) -> (Vec<f32>, Vec<f32>) {
    feed(h, level, Some(0.5), BATCH)
}

#[test]
fn the_curve_is_applied_from_each_trigger() {
    let mut h = curve_duck();
    // Quiet until the first hit: the curve is idle and the audio passes.
    let (out, _) = feed(&mut h, 0.5, Some(0.0), ms(20.0));
    assert!(out.iter().all(|x| *x == 0.5));
    let (first, _) = hit(&mut h, 0.5);
    let (rest, cv) = feed(&mut h, 0.5, Some(0.0), QUARTER * 2);
    let mut gain: Vec<f32> = first.iter().chain(&rest).map(|x| x / 0.5).collect();
    // The hit is the first sample of `first`: the curve starts at its first point.
    assert_eq!(gain[0], 1.0);
    // A tenth of the way (2 400 frames) the dip is full.
    assert!(gain[2_400].abs() < 1e-3, "{}", gain[2_400]);
    // Half way between the top and the end (55 percent) it is half.
    assert!((gain[13_200] - 0.5).abs() < 2e-3, "{}", gain[13_200]);
    // A quarter note on, the curve is over and holds its end.
    assert!((gain[24_000] - 1.0).abs() < 1e-3);
    assert_eq!(gain[QUARTER + 5_000], 1.0);
    // The CV is the dip, with the depth.
    assert!((cv[13_200 - BATCH] - 0.5).abs() < 2e-3);
    // A second hit later starts the same curve again, bit for bit.
    let mut again = Vec::new();
    let (a, _) = hit(&mut h, 0.5);
    again.extend(a);
    let (b, _) = feed(&mut h, 0.5, Some(0.0), QUARTER);
    again.extend(b);
    gain.truncate(QUARTER);
    let again: Vec<f32> = again.iter().map(|x| x / 0.5).collect();
    for j in [100usize, 2_400, 9_000, 13_200, 20_000] {
        assert_eq!(again[j].to_bits(), gain[j].to_bits(), "frame {j}");
    }
}

#[test]
fn with_no_points_the_curve_is_the_built_in_one() {
    let mut h = curve_duck();
    h.set_param(COUNT_KEY, 0.0);
    hit(&mut h, 0.5);
    let (_, cv) = feed(&mut h, 0.5, Some(0.0), QUARTER);
    let peak = cv.iter().copied().fold(0.0, f32::max);
    assert!(peak > 0.98, "{peak}");
    // Ducking at four percent of the length, eased out afterwards.
    let top = cv.iter().position(|v| *v >= peak - 1e-3).unwrap() + BATCH;
    assert!((top as f32 - 0.04 * QUARTER as f32).abs() < 600.0, "{top}");
    assert!(cv[QUARTER - BATCH * 3] < 0.05);
}

#[test]
fn a_bend_changes_the_way_back() {
    let mut h = curve_duck();
    draw(&mut h, &[(0.0, 0.0, 0), (0.1, 1.0, -64), (1.0, 0.0, 0)]);
    hit(&mut h, 0.5);
    let (_, cv) = feed(&mut h, 0.5, Some(0.0), QUARTER);
    // The fall of segment two is a straight line: half way through it is a half.
    // The bend of point 2 shapes the segment that STARTS at point 2.
    let mid = cv[13_200 - 2 * BATCH];
    assert!(mid < 0.5 - 0.1, "eased out recovery is further down: {mid}");
}

#[test]
fn a_retrigger_continues_from_the_playing_dip_and_never_lifts_the_gain() {
    let mut h = curve_duck();
    hit(&mut h, 0.5);
    // Half way down the way back (a dip of 0.5).
    let (_, before) = feed(&mut h, 0.5, Some(0.0), 13_200 - BATCH);
    let playing = before[before.len() - 1];
    assert!((playing - 0.5).abs() < 0.01, "{playing}");
    let (_, after) = hit(&mut h, 0.5);
    // The new curve enters where it reaches that dip, so the first dip is the playing one and
    // never lower.
    assert!(
        after[0] >= playing - 1e-3,
        "gain lifted: {} after {playing}",
        after[0]
    );
    assert!(after[0] < playing + 0.05, "{}", after[0]);
    // From there the dip only grows until the top.
    let (_, more) = feed(&mut h, 0.5, Some(0.0), ms(40.0));
    let all: Vec<f32> = after.iter().chain(&more).copied().collect();
    let top = all
        .iter()
        .position(|v| *v >= 0.999)
        .unwrap_or(all.len() - 1);
    for w in all[..=top].windows(2) {
        assert!(w[1] >= w[0] - 1e-6, "{} then {}", w[0], w[1]);
    }
}

#[test]
fn a_double_kick_never_jumps_the_gain_up() {
    let mut h = curve_duck();
    let mut gain: Vec<f32> = Vec::new();
    let (g, _) = hit(&mut h, 0.5);
    gain.extend(g);
    // 5 ms to 150 ms later, a second hit, at several spacings.
    for gap in [ms(5.0), ms(20.0), ms(60.0), ms(150.0)] {
        let (g, _) = feed(&mut h, 0.5, Some(0.0), gap);
        gain.extend(g);
        let (g, _) = hit(&mut h, 0.5);
        gain.extend(g);
    }
    // The gain may fall as fast as the curve says, and it rises only slowly (the way back of a
    // quarter note takes 21 600 frames): no step up beyond what the straight line allows.
    let slope = 1.0 / (0.9 * QUARTER as f32);
    for w in gain.windows(2) {
        assert!(
            w[1] - w[0] <= 2.0 * slope * 0.5 + 1e-6,
            "{} then {}",
            w[0],
            w[1]
        );
    }
}

#[test]
fn notes_trigger_the_curve_and_the_ducked_signal_does_not() {
    let mut h = curve_duck();
    h.set_param("trigger", 1.0);
    // The main input is loud, the notes are silent: nothing triggers.
    let (out, _) = feed(&mut h, 0.5, None, ms(30.0));
    assert!(out.iter().all(|x| *x == 0.5));
    // A note-on at frame 10 of a batch starts the curve there.
    h.push_note("notes_in", on(10, 36, 1.0));
    let (out, _) = feed(&mut h, 0.5, None, QUARTER);
    assert_eq!(out[9], 0.5);
    assert_eq!(out[10], 0.5, "the curve starts at its first point");
    assert!(out[10 + 2_400].abs() < 1e-3);
    assert!((out[10 + 13_200] - 0.25).abs() < 2e-3);
}

#[test]
fn the_length_follows_the_note_value_the_tempo_and_the_meter() {
    // An eighth note: half as long.
    let mut h = curve_duck();
    h.set_param("length", 1.0);
    hit(&mut h, 0.5);
    let (out, _) = feed(&mut h, 0.5, Some(0.0), QUARTER);
    // The top is at a tenth of 12 000 frames.
    let top = out.iter().position(|x| *x < 1e-3).unwrap();
    assert!((top as f32 - 1_200.0).abs() < 200.0, "{top}");
    // One bar of 4/4 at 120 BPM: 96 000 frames.
    let mut h = curve_duck();
    h.set_param("length", 4.0);
    hit(&mut h, 0.5);
    let (out, _) = feed(&mut h, 0.5, Some(0.0), 12_000);
    let top = out.iter().position(|x| *x < 1e-3).unwrap();
    assert!((top as f32 - 9_600.0).abs() < 200.0, "{top}");
    // A bar of 3/4 is a quarter shorter.
    let mut h = curve_duck();
    h.set_param("length", 4.0);
    h.time_mut().time_signature = (3, 4);
    hit(&mut h, 0.5);
    let (out, _) = feed(&mut h, 0.5, Some(0.0), 12_000);
    let top = out.iter().position(|x| *x < 1e-3).unwrap();
    assert!((top as f32 - 7_200.0).abs() < 200.0, "{top}");
    // Twice the tempo: the same note value takes half the frames.
    let mut h = curve_duck();
    let tpf = h.time_mut().ticks_per_frame;
    h.time_mut().ticks_per_frame = tpf * 2.0;
    hit(&mut h, 0.5);
    let (out, _) = feed(&mut h, 0.5, Some(0.0), QUARTER);
    let top = out.iter().position(|x| *x < 1e-3).unwrap();
    assert!((top as f32 - 1_200.0).abs() < 200.0, "{top}");
}

#[test]
fn the_curve_follows_the_depth_and_send_leaves_the_audio() {
    let mut h = curve_duck();
    h.set_param("depth", 0.5);
    hit(&mut h, 0.5);
    let (out, cv) = feed(&mut h, 0.5, Some(0.0), 3_000);
    assert!((out[2_400] - 0.25).abs() < 2e-3, "{}", out[2_400]);
    assert!((cv[2_400] - 0.5).abs() < 2e-3);
    let mut h = curve_duck();
    h.set_param("send", 1.0);
    hit(&mut h, 0.5);
    let (out, cv) = feed(&mut h, 0.5, Some(0.0), 3_000);
    assert!(out.iter().all(|x| *x == 0.5));
    assert!(cv[2_400 - BATCH] > 0.95);
}

#[test]
fn curve_mode_renders_the_same_bits_every_time() {
    let run = || {
        let mut h = curve_duck();
        let mut all = Vec::new();
        for k in 0..6 {
            all.extend(
                feed(
                    &mut h,
                    0.37,
                    Some(if k % 2 == 0 { 0.5 } else { 0.0 }),
                    20_000,
                )
                .0,
            );
        }
        all
    };
    let (a, b) = (run(), run());
    assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
    assert!(a.iter().any(|x| *x < 0.3));
    // The device the registry builds (the bypass wrapper) gives the same.
    let mut w = DeviceHarness::new((factory().create)(), SR);
    w.set_param("mode", 3.0);
    w.set_param("depth", 1.0);
    draw(&mut w, &[(0.0, 0.0, 0), (0.1, 1.0, 0), (1.0, 0.0, 0)]);
    let (wrapped, _) = feed(&mut w, 0.37, Some(0.5), 20_000);
    assert!(wrapped.iter().any(|x| *x < 0.1));
}

#[test]
fn the_curve_params_alone_decide_the_curve() {
    // Setting the points in another order, or twice, gives the same curve.
    let run = |order: &[usize]| {
        let mut h = duck();
        h.set_param("mode", 3.0);
        h.set_param("depth", 1.0);
        let pts = [(0.0, 0.0, 0i8), (0.1, 1.0, 0), (1.0, 0.0, 0)];
        for &i in order {
            h.set_param(POINT_KEYS[i][0], pts[i].0);
            h.set_param(POINT_KEYS[i][1], pts[i].1);
            h.set_param(POINT_KEYS[i][2], f32::from(pts[i].2));
        }
        h.set_param(COUNT_KEY, 3.0);
        hit(&mut h, 0.5);
        feed(&mut h, 0.5, Some(0.0), 5_000).0
    };
    let a = run(&[0, 1, 2]);
    let b = run(&[2, 0, 1, 2, 1]);
    assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
}

#[test]
fn meters_report_the_dip_and_the_position() {
    let mut h = curve_duck();
    hit(&mut h, 0.5);
    feed(&mut h, 0.5, Some(0.0), 2_400 - BATCH);
    let mut v = [0.0f32; METERS];
    assert_eq!(h.device().meter_values(&mut v), METERS);
    assert!(v[meter::DIP] > 0.9, "{v:?}");
    assert!((v[meter::POSITION] - 0.1).abs() < 0.01, "{v:?}");
    // Outside curve mode the position reads 1.
    let mut p = duck();
    feed(&mut p, 0.5, None, BATCH);
    p.device().meter_values(&mut v);
    assert_eq!(v[meter::POSITION], 1.0);
}

// ------------------------------------------------------------------ band mode

fn sine(freq: f32, n: usize) -> f32 {
    0.5 * (std::f32::consts::TAU * freq * n as f32 / SR as f32).sin()
}

/// The device in band mode with a key hit every `period` frames, rendering `frames` of the input
/// `signal`; returns the left output.
fn band_run(signal: impl Fn(usize) -> f32, depth: f32, band: bool, frames: usize) -> Vec<f32> {
    let mut h = duck();
    h.set_param("mode", 1.0);
    h.set_param("depth", depth);
    h.set_param("attack", 1.0);
    h.set_param("time", 2_000.0);
    h.set_param("band", if band { 1.0 } else { 0.0 });
    h.set_param("band_freq", 120.0);
    let mut out = Vec::new();
    for b in 0..frames / BATCH {
        let mut x = [0.0f32; BATCH];
        for (i, v) in x.iter_mut().enumerate() {
            *v = signal(b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        // The key hits in the first batch only; the sidechain is cabled.
        let key = if b == 0 { 0.5 } else { 0.0 };
        *h.input_mut("sc_in_l") = [key; BATCH];
        *h.input_mut("sc_in_r") = [key; BATCH];
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    out
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn band_off_is_the_plain_ducker_and_band_on_ducks_only_the_low_band() {
    let low = |n| sine(50.0, n);
    let high = |n| sine(6_000.0, n);
    let frames = 48_000;
    // Band off: everything is ducked together.
    let off = band_run(|n| low(n) + high(n), 1.0, false, frames);
    // The dip is full from about 1 ms and the time is two seconds, so by 0.4 s it is still
    // nearly full: the whole signal is nearly gone.
    assert!(
        rms(&off[19_200..24_000])
            < 0.3
                * rms(&{
                    let v: Vec<f32> = (0..4_800)
                        .map(|n| low(19_200 + n) + high(19_200 + n))
                        .collect();
                    v
                })
    );
    // Band on: the low band goes, the highs stay.
    let on = band_run(|n| low(n) + high(n), 1.0, true, frames);
    let want_high: Vec<f32> = (19_200..24_000).map(high).collect();
    // A 50 Hz tone is under the split by more than an octave: ducked by more than 20 dB at
    // the depth of about 0.8 left of the dip by then (the time is two seconds).
    let low_only = band_run(low, 1.0, true, frames);
    let before = rms(&(19_200..24_000).map(low).collect::<Vec<_>>());
    assert!(
        rms(&low_only[19_200..24_000]) < 0.35 * before,
        "{} of {before}",
        rms(&low_only[19_200..24_000])
    );
    // The highs keep their level (the pair is an allpass): the 6 kHz part of the mix is the same
    // size as the tone alone.
    let high_only = band_run(high, 1.0, true, frames);
    let r = rms(&high_only[19_200..24_000]) / rms(&want_high);
    assert!((r - 1.0).abs() < 0.01, "{r}");
    // And the high tone is not touched by the ducking: the difference from the same device
    // at rest is the leak of the low-pass, under -80 dB of the tone.
    let rest = band_run(high, 0.0, true, frames);
    for (a, b) in high_only[19_200..24_000].iter().zip(&rest[19_200..24_000]) {
        assert!((a - b).abs() < 5e-5, "{a} {b}");
    }
    assert!(rms(&on[19_200..24_000]) > rms(&want_high) * 0.9);
}

#[test]
fn band_on_with_nothing_ducking_keeps_every_level() {
    // An allpass: the level of every frequency is the input's.
    for f in [40.0, 120.0, 400.0, 3_000.0, 12_000.0] {
        let out = band_run(|n| sine(f, n), 0.0, true, 24_000);
        let (a, b) = (rms(&out[12_000..24_000]), 0.5 / 2f32.sqrt());
        assert!((a / b - 1.0).abs() < 0.01, "{f} Hz: {a} vs {b}");
    }
}

#[test]
fn band_off_is_still_the_input_bit_for_bit_at_rest() {
    let out = band_run(|n| sine(1_000.0, n), 0.0, false, 4_800);
    for (n, y) in out.iter().enumerate() {
        assert_eq!(y.to_bits(), sine(1_000.0, n).to_bits());
    }
}

#[test]
fn band_mode_gives_the_same_bits_every_time_and_keeps_the_cv() {
    let a = band_run(|n| sine(60.0, n) + sine(5_000.0, n), 0.9, true, 12_000);
    let b = band_run(|n| sine(60.0, n) + sine(5_000.0, n), 0.9, true, 12_000);
    assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
    // The CV out is the same with or without the band: it is the dip.
    let mut h = duck();
    h.set_param("mode", 1.0);
    h.set_param("band", 1.0);
    h.set_param("depth", 1.0);
    let (_, cv) = feed(&mut h, 0.5, None, ms(10.0));
    assert!(cv.iter().cloned().fold(0.0, f32::max) > 0.9);
}

#[test]
fn band_frequency_sets_the_split() {
    let run = |freq: f32| {
        let mut h = duck();
        h.set_param("mode", 1.0);
        h.set_param("depth", 1.0);
        h.set_param("attack", 1.0);
        h.set_param("time", 2_000.0);
        h.set_param("band", 1.0);
        h.set_param("band_freq", freq);
        let mut out = Vec::new();
        for b in 0..300 {
            let mut x = [0.0f32; BATCH];
            for (i, v) in x.iter_mut().enumerate() {
                *v = sine(200.0, b * BATCH + i);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            let key = if b == 0 { 0.5 } else { 0.0 };
            *h.input_mut("sc_in_l") = [key; BATCH];
            *h.input_mut("sc_in_r") = [key; BATCH];
            h.process();
            out.extend_from_slice(h.output("out_l"));
        }
        rms(&out[9_600..19_200])
    };
    // A 200 Hz tone sits under a 500 Hz split (ducked) and over an 80 Hz one (left alone).
    assert!(run(500.0) < 0.5 * run(80.0));
}

#[test]
fn curve_mode_without_a_sidechain_or_notes_listens_to_the_main_input() {
    let mut h = curve_duck();
    let (_, cv) = feed(&mut h, 0.5, None, ms(20.0));
    assert!(cv.iter().cloned().fold(0.0, f32::max) > 0.0);
    let mut w = DeviceHarness::new((factory().create)(), SR);
    w.set_param("mode", 3.0);
    w.set_param("depth", 1.0);
    let (_, cv) = feed(&mut w, 0.5, None, ms(20.0));
    assert!(cv.iter().cloned().fold(0.0, f32::max) > 0.0);
}

#[test]
fn the_trigger_choice_picks_the_key_the_notes_or_both() {
    let cv_peak = |trigger: f32, key: Option<f32>, note: bool| {
        let mut h = curve_duck();
        h.set_param("trigger", trigger);
        if note {
            h.push_note("notes_in", on(0, 36, 1.0));
        }
        let (_, cv) = feed(&mut h, 0.0, key, ms(20.0));
        cv.iter().copied().fold(0.0, f32::max)
    };
    // The key alone: the key triggers, notes do not.
    assert!(cv_peak(0.0, Some(0.5), false) > 0.2);
    assert!(cv_peak(0.0, Some(0.0), true) < 0.01);
    // Notes alone: the loud key does not.
    assert!(cv_peak(1.0, Some(0.5), false) < 0.01);
    assert!(cv_peak(1.0, Some(0.0), true) > 0.2);
    // Either.
    assert!(cv_peak(2.0, Some(0.5), false) > 0.2);
    assert!(cv_peak(2.0, Some(0.0), true) > 0.2);
}
