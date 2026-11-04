//! Tuner device tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, run_signal, sine, sweep_params_min_max};
use atomik_dsp::tuner::{self, Tuner as Analysis};
use atomik_testkit::harness::DeviceHarness;

fn tuner() -> DeviceHarness {
    harness(Box::new(Tuner::new()))
}

fn ms(t: f64) -> usize {
    (t * 0.001 * SR).round() as usize
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &["mute", "ref", "bypass"],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.tuner");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    let r = DESCRIPTOR.param_by_key("ref").unwrap();
    assert_eq!((r.min, r.max, r.default), (415.0, 466.0, 440.0));
}

#[test]
fn the_audio_passes_bit_for_bit_with_no_latency() {
    let mut h = tuner();
    let tone = sine(220.0, 0.7);
    let y = run_signal(&mut h, &["in_l", "in_r"], &tone, 40, &["out_l", "out_r"]);
    for (c, ch) in y.iter().enumerate() {
        for (n, v) in ch.iter().enumerate() {
            assert_eq!(v.to_bits(), tone(n).to_bits(), "channel {c} sample {n}");
        }
    }
}

#[test]
fn mute_ramps_the_output_down_in_five_ms_and_back_up() {
    let mut h = tuner();
    h.set_param("mute", 1.0);
    let y = run_signal(&mut h, &["in_l", "in_r"], |_| 1.0, 40, &["out_l"]).remove(0);
    assert!(y[0] < 1.0 && y[0] > 0.9, "starts to fall at once");
    assert!(y[ms(2.0)] > 0.3 && y[ms(2.0)] < 0.7, "{}", y[ms(2.0)]);
    assert_eq!(y[ms(5.0) + 2], 0.0);
    assert!(y[ms(5.0) + 2..].iter().all(|v| *v == 0.0));
    h.set_param("mute", 0.0);
    let y = run_signal(&mut h, &["in_l", "in_r"], |_| 1.0, 40, &["out_l"]).remove(0);
    assert!(y[0] > 0.0 && y[0] < 0.1);
    assert_eq!(y[ms(5.0) + 2], 1.0);
}

#[test]
fn the_tap_hears_the_input_even_while_muted() {
    let mut h = tuner();
    h.set_param("mute", 1.0);
    let tone = sine(440.0, 0.5);
    run_signal(&mut h, &["in_l", "in_r"], &tone, 200, &["out_l"]);
    let tap = h.device().scope_tap().expect("a tap");
    let (mut l, mut r) = (vec![0.0f32; 4096], vec![0.0f32; 4096]);
    let (n, head) = tap.read_latest(&mut l, &mut r);
    assert_eq!(n, 4096);
    assert_eq!(head, 200 * BATCH as u64);
    let first = head as usize - n;
    for (i, v) in l.iter().enumerate() {
        assert_eq!(v.to_bits(), tone(first + i).to_bits(), "frame {i}");
    }
}

#[test]
fn the_analysis_of_the_tap_finds_the_note_and_the_cents() {
    // 110 Hz played 20 cents sharp of A2 is read against the 440 Hz reference.
    let mut h = tuner();
    let hz = 110.0 * 2f64.powf(20.0 / 1200.0);
    run_signal(&mut h, &["in_l", "in_r"], sine(hz, 0.5), 400, &["out_l"]);
    let tap = h.device().scope_tap().unwrap();
    let want = tuner::frames_wanted(SR as f32);
    let (mut l, mut r) = (vec![0.0f32; want], vec![0.0f32; want]);
    let (n, _) = tap.read_latest(&mut l, &mut r);
    let reading = Analysis::new()
        .update(&l[..n], SR as f32, 0, 440.0)
        .expect("a note");
    assert_eq!(tuner::key_name(reading.note.key), "A");
    assert!(
        (reading.note.cents - 20.0).abs() < 1.0,
        "{}",
        reading.note.cents
    );
    // Against a reference 20 cents sharp it is in tune.
    let flat = 440.0 * 2f32.powf(20.0 / 1200.0);
    let reading = Analysis::new().update(&l[..n], SR as f32, 0, flat).unwrap();
    assert!(reading.in_tune(), "{}", reading.note.cents);
}

#[test]
fn non_finite_input_counts_as_silence_on_the_outputs_and_the_tap() {
    let mut h = tuner();
    let y = run_signal(
        &mut h,
        &["in_l", "in_r"],
        |n| if n % 3 == 0 { f32::NAN } else { f32::INFINITY },
        10,
        &["out_l"],
    );
    assert!(y[0].iter().all(|v| *v == 0.0));
    let tap = h.device().scope_tap().unwrap();
    let (mut l, mut r) = (vec![1.0f32; 256], vec![1.0f32; 256]);
    tap.read_latest(&mut l, &mut r);
    assert!(l.iter().chain(&r).all(|v| *v == 0.0));
}

#[test]
fn every_parameter_extreme_stays_finite() {
    let mut h = tuner();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
