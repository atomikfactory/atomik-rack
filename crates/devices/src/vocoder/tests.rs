//! Vocoder tests.

use super::*;
use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max, tone_amplitude};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::rms;
use proptest::prelude::{ProptestConfig, prop_assert, proptest};

fn vocoder() -> DeviceHarness {
    harness(Box::new(Vocoder::new()))
}

/// Feeds a modulator and a carrier (both stereo-identical) and returns the left output.
fn run(h: &mut DeviceHarness, modulator: &[f32], carrier: &[f32]) -> Vec<f32> {
    h.set_connected("carrier_l", true);
    h.set_connected("carrier_r", true);
    let mut out = Vec::new();
    for (m, c) in modulator.chunks(BATCH).zip(carrier.chunks(BATCH)) {
        let (mut bm, mut bc) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        bm[..m.len()].copy_from_slice(m);
        bc[..c.len()].copy_from_slice(c);
        *h.input_mut("in_l") = bm;
        *h.input_mut("in_r") = bm;
        *h.input_mut("carrier_l") = bc;
        *h.input_mut("carrier_r") = bc;
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    out
}

fn sine(hz: f64, amp: f64, n: usize) -> Vec<f32> {
    let f = crate::test_support::sine(hz, amp);
    (0..n).map(f).collect()
}

fn noise(seed: u64, n: usize, amp: f32) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..n).map(|_| r.next_bipolar() * amp).collect()
}

/// Centre of band `i` of 16 with the default range.
fn centre(i: usize) -> f64 {
    (100.0f64 * (8_000.0f64 / 100.0).powf(i as f64 / 15.0)).round()
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "bands",
            "low",
            "high",
            "bandwidth",
            "attack",
            "release",
            "hiss",
            "mix",
            "output",
            "bypass",
        ],
        &["in_l", "in_r", "carrier_l", "carrier_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.vocoder");
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
}

#[test]
fn a_silent_modulator_gives_silence() {
    let mut h = vocoder();
    let n = 20_000;
    let out = run(&mut h, &vec![0.0; n], &noise(1, n, 0.5));
    assert!(out.iter().all(|v| v.abs() < 1e-6));
}

#[test]
fn the_carrier_comes_through_in_the_band_the_modulator_excites() {
    let n = SR as usize;
    for band in [3usize, 8, 12] {
        let f = centre(band);
        let out_at = |mod_hz: f64, car_hz: f64| {
            let mut h = vocoder();
            h.set_param("hiss", 0.0);
            let y = run(&mut h, &sine(mod_hz, 0.5, n), &sine(car_hz, 0.5, n));
            tone_amplitude(&y[n / 2..], car_hz, n / 2)
        };
        let matched = out_at(f, f);
        // The follower of a 0.5 sine times the carrier amplitude 0.5 is about 0.25; the neighbouring bands add a little.
        assert!((0.2..0.45).contains(&matched), "band {band}: {matched}");
        // A modulator far above the band, or a carrier far from the modulator, leaves it quiet.
        assert!(
            out_at(f * 6.0, f) < matched * 0.05,
            "band {band} modulator off"
        );
        assert!(
            out_at(f, f * 6.0) < matched * 0.05,
            "band {band} carrier off"
        );
    }
}

#[test]
fn more_bands_follow_the_spectrum_more_closely() {
    // A modulator with energy only at one frequency passes less of a noise carrier with
    // more (narrower) bands.
    let n = 48_000;
    let level = |bands: f32| {
        let mut h = vocoder();
        h.set_param("hiss", 0.0);
        h.set_param("bands", bands);
        let y = run(&mut h, &sine(1_000.0, 0.5, n), &noise(2, n, 0.5));
        rms(&y[n / 2..])
    };
    assert!(
        level(32.0) < level(4.0) * 0.8,
        "{} vs {}",
        level(32.0),
        level(4.0)
    );
}

#[test]
fn the_hiss_follows_the_high_frequency_content() {
    let n = 30_000;
    let level = |hiss: f32| {
        let mut h = vocoder();
        h.set_param("hiss", hiss);
        h.set_param("bands", 8.0);
        // No external carrier on the right, a noise burst on the modulator.
        h.set_connected("carrier_l", false);
        h.set_connected("carrier_r", false);
        let m = noise(3, n, 0.5);
        let mut out = Vec::new();
        for chunk in m.chunks(BATCH) {
            let mut b = [0.0f32; BATCH];
            b[..chunk.len()].copy_from_slice(chunk);
            *h.input_mut("in_l") = b;
            *h.input_mut("in_r") = b;
            h.process();
            out.extend_from_slice(h.output("out_l"));
        }
        rms(&out[n / 2..])
    };
    assert!(level(1.0) > level(0.0) * 1.2);
}

#[test]
fn mix_zero_is_dry_bit_exact_with_no_latency() {
    let n = 10_000;
    let x = noise(9, n, 1.0);
    let mut h = vocoder();
    h.set_param("mix", 0.0);
    let out = run(&mut h, &x, &noise(10, n, 1.0));
    assert!(out.iter().zip(&x).all(|(a, b)| a.to_bits() == b.to_bits()));
    assert_eq!(h.device().latency_frames(), 0);
}

#[test]
fn deterministic_and_allocation_free_while_bands_change() {
    let n = 40_000;
    let (m, c) = (noise(4, n, 0.5), noise(5, n, 0.5));
    let go = || {
        let mut h = vocoder();
        let mut out = Vec::new();
        for (b, (mc, cc)) in m.chunks(BATCH).zip(c.chunks(BATCH)).enumerate() {
            if b % 50 == 0 {
                h.set_param("bands", (4 + b / 50 % 29) as f32);
                h.set_param("low", 50.0 + (b % 7) as f32 * 40.0);
                h.set_param("high", 3_000.0 + (b % 5) as f32 * 2_500.0);
            }
            let (mut bm, mut bc) = ([0.0f32; BATCH], [0.0f32; BATCH]);
            bm[..mc.len()].copy_from_slice(mc);
            bc[..cc.len()].copy_from_slice(cc);
            *h.input_mut("in_l") = bm;
            *h.input_mut("in_r") = bm;
            *h.input_mut("carrier_l") = bc;
            h.set_connected("carrier_l", true);
            h.process(); // guarded by the harness
            out.extend_from_slice(h.output("out_l"));
        }
        out
    };
    let (a, b) = (go(), go());
    assert!(a.iter().all(|v| v.is_finite()));
    assert_eq!(
        a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        b.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn every_param_min_max_does_not_panic() {
    let mut h = vocoder();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn output_is_finite_and_bounded_for_any_params(
        bands in 4.0f32..32.0,
        low in 40.0f32..500.0,
        high in 2_000.0f32..16_000.0,
        bandwidth in 0.5f32..2.0,
        attack in 0.1f32..50.0,
        release in 1.0f32..500.0,
        hiss in 0.0f32..1.0,
        output in -24.0f32..12.0,
        seed in 0u64..100,
    ) {
        let mut h = vocoder();
        for (k, v) in [
            ("bands", bands), ("low", low), ("high", high), ("bandwidth", bandwidth),
            ("attack", attack), ("release", release), ("hiss", hiss), ("output", output),
        ] {
            h.set_param(k, v);
        }
        let y = run(&mut h, &noise(seed, 30 * BATCH, 1.0), &noise(seed + 1, 30 * BATCH, 1.0));
        for v in &y {
            prop_assert!(v.is_finite() && v.abs() <= 32.0 * 4.0 + 1.0, "{v}");
        }
    }
}
