//! Sidechain tool tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max};
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
    assert_keys(
        &DESCRIPTOR,
        &[
            "mode",
            "rate",
            "depth",
            "attack",
            "time",
            "curve",
            "threshold",
            "send",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "sc_in_l",
            "sc_in_r",
            "out_l",
            "out_r",
            "duck_cv_out",
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
    for mode in 0..3 {
        let mut h = duck();
        h.set_param("mode", mode as f32);
        *h.input_mut("in_l") = [0.9; BATCH];
        *h.input_mut("in_r") = [-0.3; BATCH];
        *h.input_mut("sc_in_l") = [0.7; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }
}
