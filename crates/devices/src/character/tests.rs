//! Device tests of `atomik.character` for the shell and all 16
//! presets; the per-preset character checks of presets 8 to 15 are in `tests/row34.rs`.

use super::chain::{
    Ctx, PresetChain, Routing, SUB, StereoDelay, StubChain, long_latency, short_latency,
};
use super::voicing::{ChainSpec, Morph, PresetVoicing, Voicing};
use super::*;
use crate::test_support::{SR, assert_keys, sweep_params_min_max};
use atomik_core::BATCH;
use atomik_dsp::{PinkNoise, Rng};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use proptest::prelude::{Just, ProptestConfig, prop_assert, prop_oneof, proptest};

fn dev_at(sr: f64) -> DeviceHarness {
    DeviceHarness::with_options(Box::new(Character::new()), sr, 120.0, 11)
}

fn dev() -> DeviceHarness {
    dev_at(SR)
}

/// Feeds `l` (and `r`, or `l` again) in batches and returns both outputs.
fn run(h: &mut DeviceHarness, l: &[f32], r: Option<&[f32]>) -> (Vec<f32>, Vec<f32>) {
    assert_eq!(l.len() % BATCH, 0);
    let (mut ol, mut or) = (Vec::with_capacity(l.len()), Vec::with_capacity(l.len()));
    for (i, chunk) in l.chunks(BATCH).enumerate() {
        h.input_mut("in_l").copy_from_slice(chunk);
        match r {
            Some(r) => h
                .input_mut("in_r")
                .copy_from_slice(&r[i * BATCH..(i + 1) * BATCH]),
            None => h.input_mut("in_r").copy_from_slice(chunk),
        }
        h.process();
        ol.extend_from_slice(h.output("out_l"));
        or.extend_from_slice(h.output("out_r"));
    }
    (ol, or)
}

fn noise(n: usize, seed: u64, amp: f32) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..n).map(|_| r.next_bipolar() * amp).collect()
}

fn pink(n: usize, seed: u64, rms: f32) -> Vec<f32> {
    let mut p = PinkNoise::new(seed);
    (0..n).map(|_| p.next_sample() * rms).collect()
}

fn sine(n: usize, hz: f64, amp: f64, sr: f64) -> Vec<f32> {
    (0..n)
        .map(|i| (amp * (std::f64::consts::TAU * hz * i as f64 / sr).sin()) as f32)
        .collect()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

fn set_preset(h: &mut DeviceHarness, i: usize) {
    h.set_param("preset", i as f32);
}

fn latency(h: &DeviceHarness) -> usize {
    h.device().latency_frames() as usize
}

fn meters(h: &DeviceHarness) -> [f32; 4] {
    let mut m = [0.0; 4];
    assert_eq!(h.device().meter_values(&mut m), 4);
    m
}

// --- descriptor and robustness ----------------------------------------------------------

#[test]
fn descriptor_validates_and_pins_keys_ids_and_order() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    assert_eq!(DESCRIPTOR.type_id, "atomik.character");
    assert_eq!(DESCRIPTOR.name, "Character");
    assert_eq!(DESCRIPTOR.kind, atomik_device_sdk::DeviceKind::Effect);
    assert_keys(
        &DESCRIPTOR,
        &[
            "gain",
            "preset",
            "transform",
            "mix",
            "volume",
            "transform_cv_trim",
            "mix_cv_trim",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "transform_cv_in",
            "mix_cv_in",
            "out_l",
            "out_r",
        ],
    );
    use atomik_device_sdk::{Direction, SocketKind};
    let kinds: Vec<_> = DESCRIPTOR
        .sockets
        .iter()
        .map(|s| (s.kind, s.direction))
        .collect();
    assert_eq!(
        kinds,
        [
            (SocketKind::Audio, Direction::Input),
            (SocketKind::Audio, Direction::Input),
            (SocketKind::Cv, Direction::Input),
            (SocketKind::Cv, Direction::Input),
            (SocketKind::Audio, Direction::Output),
            (SocketKind::Audio, Direction::Output),
        ]
    );
    let defaults: Vec<f32> = DESCRIPTOR.params.iter().map(|p| p.default).collect();
    assert_eq!(defaults, [0.5, 0.0, 0.5, 1.0, 0.5, 1.0, 1.0, 0.0]);
    let preset = &DESCRIPTOR.params[1];
    assert!(preset.discrete);
    assert!((preset.min - 0.0).abs() < f32::EPSILON && (preset.max - 15.0).abs() < f32::EPSILON);
    match preset.unit {
        atomik_core::Unit::Choice(names) => assert_eq!(names.len(), 16),
        _ => panic!("preset is a choice"),
    }
    assert!(Character::new().has_meter_values());
}

#[test]
fn gain_and_volume_follow_four_x_squared() {
    assert!((knob_gain(0.5) - 1.0).abs() < 1e-7);
    assert_eq!(knob_gain(0.5).to_bits(), 1.0f32.to_bits(), "exactly 0 dB");
    assert!(knob_gain(0.0).abs() < f32::MIN_POSITIVE);
    for i in 0..=10 {
        let x = i as f32 / 10.0;
        let want = 4.0 * f64::from(x) * f64::from(x);
        let got = f64::from(knob_gain(x));
        if i == 0 {
            assert!(got.abs() < 1e-12);
        } else {
            assert!((db(got) - db(want)).abs() < 0.001, "x={x}");
        }
    }
    assert!((knob_gain_db(1.0) - 12.0412).abs() < 0.001);
    assert!(knob_gain_db(0.0).is_infinite() && knob_gain_db(0.0) < 0.0);
    assert!(knob_gain(f32::NAN).is_finite());
    assert!((knob_gain(7.0) - 4.0).abs() < 1e-6, "clamped");
}

#[test]
fn set_param_and_reset_are_allocation_free() {
    let mut h = dev();
    for p in DESCRIPTOR.params {
        for v in [p.min, p.max, p.default] {
            assert_no_alloc(|| h.set_param(p.key, v));
            h.process();
        }
    }
    assert_no_alloc(|| h.device_mut().reset());
    h.process();
}

#[test]
fn every_param_at_min_max_and_default_keeps_the_output_finite() {
    let mut h = dev();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[derive(Debug, Clone)]
enum Pattern {
    Silence,
    Noise,
    FullScale,
    Nan,
    Inf,
    Denormal,
}

fn pattern(p: &Pattern, n: usize, seed: u64) -> Vec<f32> {
    match p {
        Pattern::Silence => vec![0.0; n],
        Pattern::Noise => noise(n, seed, 0.9),
        Pattern::FullScale => (0..n).map(|i| if i % 7 < 4 { 1.0 } else { -1.0 }).collect(),
        Pattern::Nan => (0..n)
            .map(|i| if i % 5 == 0 { f32::NAN } else { 0.3 })
            .collect(),
        Pattern::Inf => (0..n)
            .map(|i| match i % 9 {
                0 => f32::INFINITY,
                4 => f32::NEG_INFINITY,
                _ => 0.2,
            })
            .collect(),
        Pattern::Denormal => (0..n)
            .map(|i| f32::MIN_POSITIVE * 0.01 * (i % 3) as f32)
            .collect(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn any_params_and_input_keep_the_output_finite_and_bounded(
        preset in 0usize..16,
        transform in 0.0f32..=1.0,
        gain in 0.0f32..=1.0,
        mix in 0.0f32..=1.0,
        volume in 0.0f32..=1.0,
        tt in 0.0f32..=1.0,
        mt in 0.0f32..=1.0,
        cv in -1.0f32..=1.0,
        sr in prop_oneof![Just(44_100.0f64), Just(48_000.0), Just(96_000.0)],
        pat in prop_oneof![
            Just(Pattern::Silence), Just(Pattern::Noise), Just(Pattern::FullScale),
            Just(Pattern::Nan), Just(Pattern::Inf), Just(Pattern::Denormal)
        ],
    ) {
        let mut h = dev_at(sr);
        set_preset(&mut h, preset);
        for (k, v) in [("transform", transform), ("gain", gain), ("mix", mix), ("volume", volume),
                       ("transform_cv_trim", tt), ("mix_cv_trim", mt)] {
            h.set_param(k, v);
        }
        h.input_mut("transform_cv_in").fill(cv);
        h.input_mut("mix_cv_in").fill(-cv);
        let x = pattern(&pat, BATCH * 60, 5);
        let (l, r) = run(&mut h, &x, None);
        prop_assert!(l.iter().chain(r.iter()).all(|v| v.is_finite() && v.abs() <= 8.0));
    }
}

#[test]
fn the_tail_after_a_burst_decays_to_nothing_without_denormals() {
    for preset in 0..16 {
        let mut h = dev();
        set_preset(&mut h, preset);
        let burst = noise(BATCH * 200, 3, 0.8);
        let _ = run(&mut h, &burst, None);
        let silence = vec![0.0f32; BATCH * 750];
        let (mut worst_late, mut subnormal) = (0.0f32, 0usize);
        for k in 0..(20 * 48_000 / (BATCH * 750)) {
            let (l, r) = run(&mut h, &silence, None);
            subnormal += l
                .iter()
                .chain(r.iter())
                .filter(|v| v.is_subnormal())
                .count();
            if k >= 18 {
                worst_late = worst_late.max(peak(&l)).max(peak(&r));
            }
        }
        assert_eq!(subnormal, 0, "preset {preset}");
        assert!(worst_late < 1e-6, "preset {preset}: {worst_late}");
    }
}

// --- null, laws, gain staging -----------------------------------------------------------

#[test]
fn mix_zero_is_the_input_delayed_by_the_latency_bit_for_bit_for_any_input_gain() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        for preset in 0..16 {
            for gain in [0.0f32, 0.3, 0.5, 1.0] {
                let mut h = dev_at(sr);
                set_preset(&mut h, preset);
                h.set_param("mix", 0.0);
                h.set_param("gain", gain);
                let x = noise(BATCH * 40, 100 + preset as u64, 0.9);
                let y = noise(BATCH * 40, 200 + preset as u64, 0.9);
                let (l, r) = run(&mut h, &x, Some(&y));
                let lat = latency(&h);
                assert!(lat > 0);
                for n in 0..x.len() {
                    let (wl, wr) = if n >= lat {
                        (x[n - lat], y[n - lat])
                    } else {
                        (0.0, 0.0)
                    };
                    assert_eq!(
                        l[n].to_bits(),
                        wl.to_bits(),
                        "sr {sr} preset {preset} gain {gain} n {n}"
                    );
                    assert_eq!(
                        r[n].to_bits(),
                        wr.to_bits(),
                        "sr {sr} preset {preset} gain {gain} n {n}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_mix_is_a_linear_crossfade_of_dry_and_wet() {
    for preset in [0usize, 2, 5, 7, 9] {
        let render_at = |mix: f32| {
            let mut h = dev();
            set_preset(&mut h, preset);
            h.set_param("mix", mix);
            h.set_param("transform", 0.8);
            let x = noise(BATCH * 100, 9, 0.5);
            run(&mut h, &x, None).0
        };
        let (a, b, c) = (render_at(0.0), render_at(1.0), render_at(0.5));
        let d = render_at(0.25);
        for n in 0..a.len() {
            assert!(
                (c[n] - 0.5 * (a[n] + b[n])).abs() < 1e-6,
                "preset {preset} n {n}"
            );
            assert!(
                (d[n] - (0.75 * a[n] + 0.25 * b[n])).abs() < 1e-6,
                "preset {preset} n {n}"
            );
        }
    }
}

#[test]
fn volume_is_after_the_mix_and_follows_the_law() {
    for (vol, want) in [(0.5f32, 1.0f32), (0.25, 0.25), (1.0, 4.0), (0.0, 0.0)] {
        let mut h = dev();
        set_preset(&mut h, 1);
        h.set_param("mix", 0.0);
        h.set_param("volume", vol);
        let x = noise(BATCH * 20, 4, 0.2);
        let (l, _) = run(&mut h, &x, None);
        let lat = latency(&h);
        for n in lat..x.len() {
            assert!((l[n] - x[n - lat] * want).abs() < 1e-6, "vol {vol}");
        }
    }
}

#[test]
fn a_compensated_gain_and_volume_leave_a_linear_wet_path_unchanged() {
    // `sparkle` at Transform 0 has no exciter: a linear chain.
    let x = noise(BATCH * 300, 8, 0.1);
    let render_with = |gain: f32, vol: f32| {
        let mut h = dev();
        set_preset(&mut h, 2);
        h.set_param("transform", 0.0);
        h.set_param("gain", gain);
        h.set_param("volume", vol);
        run(&mut h, &x, None).0
    };
    let a = render_with(0.5, 0.5);
    let b = render_with(0.707_106_77, 0.353_553_4);
    let (ra, rb) = (rms(&a[4800..]), rms(&b[4800..]));
    assert!((db(ra) - db(rb)).abs() < 0.01, "{} {}", db(ra), db(rb));
}

#[test]
fn the_bottom_of_gain_silences_a_chain_without_noise_and_volume_zero_silences_everything() {
    let mut h = dev();
    set_preset(&mut h, 2);
    h.set_param("gain", 0.0);
    let x = noise(BATCH * 40, 6, 0.5);
    let (l, r) = run(&mut h, &x, None);
    assert!(
        l.iter()
            .chain(r.iter())
            .all(|v| v.abs() < f32::MIN_POSITIVE)
    );
    let mut h = dev();
    set_preset(&mut h, 7);
    h.set_param("volume", 0.0);
    let (l, _) = run(&mut h, &x, None);
    assert!(l.iter().all(|v| v.abs() < f32::MIN_POSITIVE));
}

#[test]
fn a_mono_source_on_the_left_input_sounds_on_both_sides() {
    let mut h = dev();
    set_preset(&mut h, 1);
    let x = noise(BATCH * 40, 6, 0.5);
    for chunk in x.chunks(BATCH) {
        h.input_mut("in_l").copy_from_slice(chunk);
        h.process();
    }
    let (l, r) = (h.output("out_l"), h.output("out_r"));
    assert!(peak(l) > 0.01);
    assert_eq!(l, r, "in_r unconnected: both channels carry in_l");
    // With in_r connected the channels stay independent.
    let mut h = dev();
    set_preset(&mut h, 1);
    let y = noise(BATCH * 40, 7, 0.5);
    let (l, r) = run(&mut h, &x, Some(&y));
    assert_ne!(l, r);
}

// --- latency ----------------------------------------------------------------------------

/// The preset that is in the long latency class (the spectral stage): `lossy`.
const LONG_PRESET: usize = 8;
/// Ceiling of the share of the 6 dB bound the tightest preset switch may use (see the margin
/// monitor in `all_240_ordered_preset_switches_are_click_free`).
const WORST_SWITCH_SHARE: f64 = 0.995;

#[test]
fn every_preset_declares_its_class_latency_at_every_rate() {
    for sr in [44_100.0f64, 48_000.0, 96_000.0] {
        for preset in 0..16 {
            let mut h = dev_at(sr);
            set_preset(&mut h, preset);
            h.process();
            let want = if preset == LONG_PRESET {
                long_latency(sr as f32)
            } else {
                short_latency(sr as f32)
            };
            assert_eq!(h.device().latency_frames(), want, "sr {sr} preset {preset}");
        }
    }
    assert!(short_latency(48_000.0) >= 29);
    // The long class is the short class plus the spectral frame: 1024 at 44.1 and 48 kHz,
    // 2048 at 96 kHz.
    assert_eq!(long_latency(48_000.0), short_latency(48_000.0) + 1024);
    assert_eq!(long_latency(44_100.0), short_latency(44_100.0) + 1024);
    assert_eq!(long_latency(96_000.0), short_latency(96_000.0) + 2048);
}

#[test]
fn the_wet_impulse_arrives_at_the_declared_latency_within_the_filter_delay() {
    for sr in [44_100.0f64, 48_000.0, 96_000.0] {
        for preset in 0..16 {
            let mut h = dev_at(sr);
            set_preset(&mut h, preset);
            h.set_param("transform", 0.0);
            let mut x = vec![0.0f32; BATCH * 80]; // long enough for the long class at 96 kHz
            x[0] = 0.1;
            let (l, _) = run(&mut h, &x, None);
            let lat = latency(&h);
            // The onset (first sample above 10 % of the peak): nothing but the linear-phase
            // pre-ringing of the oversampler sounds before the declared latency, and the response
            // starts within a few frames of it.
            let tol = if sr > 60_000.0 { 12 } else { 6 }; // filter delays scale with the rate
            let pk = peak(&l);
            let onset = l.iter().position(|v| v.abs() > 0.1 * pk).unwrap();
            assert!(
                onset + 4 >= lat && onset <= lat + tol,
                "sr {sr} preset {preset}: onset at {onset}, declared {lat}"
            );
            // Presets without a long resonance also put the peak within the filter delay.
            if ![14usize, 15].contains(&preset) {
                let at = l
                    .iter()
                    .enumerate()
                    .fold(0, |m, (i, v)| if v.abs() > l[m].abs() { i } else { m });
                assert!(
                    at >= lat && at <= lat + tol,
                    "sr {sr} preset {preset}: peak at {at}, declared {lat}"
                );
            }
        }
    }
}

/// A pass-through (or inverting) test chain of the short class, or of a long class `EXTRA`
/// frames above it (stands in for the spectral presets).
#[derive(Default)]
struct TestChain {
    invert: bool,
    extra: u32,
    pad: Option<StereoDelay>,
    latency: u32,
}

const EXTRA: u32 = 480;

impl PresetChain for TestChain {
    fn spec(&self) -> ChainSpec {
        StubChain::new().spec()
    }
    fn prepare(&mut self, sr: f32, _seed: u64, _v: &PresetVoicing) {
        self.latency = short_latency(sr) + self.extra;
        self.pad = Some(StereoDelay::new(self.latency as usize));
    }
    fn reset(&mut self) {
        if let Some(p) = self.pad.as_mut() {
            p.reset();
        }
    }
    fn process(&mut self, l: &mut [f32; SUB], r: &mut [f32; SUB], _m: &Morph, _c: &Ctx) {
        let s = if self.invert { -1.0 } else { 1.0 };
        if let Some(p) = self.pad.as_mut() {
            for n in 0..SUB {
                let (a, b) = p.process(l[n], r[n]);
                l[n] = a * s;
                r[n] = b * s;
            }
        }
    }
    fn latency(&self) -> u32 {
        self.latency
    }
    fn routing(&self) -> Routing {
        Routing::Stereo
    }
    fn activity(&self) -> f32 {
        0.0
    }
    fn presence(&self) -> (f32, f32) {
        (3.0, 2.0)
    }
}

fn test_chains(i: usize) -> Box<dyn PresetChain> {
    match i {
        0 => Box::new(TestChain {
            invert: true,
            ..TestChain::default()
        }),
        1 => Box::new(TestChain::default()),
        8 | 9 => Box::new(TestChain {
            extra: EXTRA,
            ..TestChain::default()
        }),
        _ => Box::new(StubChain::new()),
    }
}

fn dev_with_test_chains() -> DeviceHarness {
    DeviceHarness::with_options(Box::new(Character::with_chains(test_chains)), SR, 120.0, 11)
}

#[test]
fn a_switch_inside_a_class_never_changes_the_reported_latency() {
    let mut h = dev_with_test_chains();
    h.process();
    let l0 = h.device().latency_frames();
    for p in [1usize, 2, 0, 3, 1, 7] {
        set_preset(&mut h, p);
        for _ in 0..80 {
            h.process();
            assert_eq!(h.device().latency_frames(), l0);
        }
    }
    let mut h = dev_with_test_chains();
    set_preset(&mut h, 8);
    h.process();
    let long = h.device().latency_frames();
    assert_eq!(long, short_latency(48_000.0) + EXTRA);
    set_preset(&mut h, 9);
    for _ in 0..80 {
        h.process();
        assert_eq!(h.device().latency_frames(), long);
    }
}

#[test]
fn a_switch_across_classes_publishes_the_new_latency_once_when_the_fade_ends() {
    let mut h = dev_with_test_chains();
    h.process();
    let short = h.device().latency_frames();
    set_preset(&mut h, 8);
    let fade_batches = (0.030 * 48_000.0 / BATCH as f64).ceil() as usize;
    let mut changes = Vec::new();
    let mut last = short;
    for b in 0..(fade_batches + 20) {
        h.process();
        let now = h.device().latency_frames();
        if now != last {
            changes.push((b, now));
            last = now;
        }
    }
    assert_eq!(changes.len(), 1, "{changes:?}");
    let (at, now) = changes[0];
    assert_eq!(now, short + EXTRA);
    assert!(
        at + 1 >= fade_batches && at <= fade_batches + 1,
        "at batch {at}, fade {fade_batches}"
    );
    // And back again.
    set_preset(&mut h, 1);
    for _ in 0..fade_batches + 5 {
        h.process();
    }
    assert_eq!(h.device().latency_frames(), short);
}

#[test]
fn the_dry_path_is_aligned_with_the_wet_path_in_both_classes() {
    // The wet test chains are pure delays of their class latency, so out = dry delayed at any mix
    // exactly when the dry path is aligned (no comb).
    for preset in [1usize, 8] {
        let mut h = dev_with_test_chains();
        set_preset(&mut h, preset);
        h.set_param("mix", 0.5);
        let x = noise(BATCH * 60, 14, 0.5);
        let (l, _) = run(&mut h, &x, None);
        let lat = latency(&h);
        for n in lat..x.len() {
            assert!((l[n] - x[n - lat]).abs() < 1e-6, "preset {preset} n {n}");
        }
    }
}

#[test]
fn during_a_class_crossing_fade_the_dry_path_crossfades_between_the_two_delays() {
    // Mix 0, preset 1 (short) to 8 (long): the output is a click-free blend of two delayed copies
    // and ends on the long class delay.
    let mut h = dev_with_test_chains();
    h.set_param("mix", 0.0);
    let x = sine(BATCH * 120, 500.0, 0.5, SR);
    let _ = run(&mut h, &x[..BATCH * 40], None);
    set_preset(&mut h, 8);
    let (l, _) = run(&mut h, &x[BATCH * 40..], None);
    let step = l.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
    assert!(
        step < 0.5 * 2.0 * std::f32::consts::PI * 500.0 / 48_000.0 * 2.0,
        "click: {step}"
    );
    let long = latency(&h);
    assert_eq!(long as u32, short_latency(48_000.0) + EXTRA);
    let tail = &l[l.len() - 200..];
    let start = BATCH * 120 - 200;
    for (i, v) in tail.iter().enumerate() {
        assert!((v - x[start + i - long]).abs() < 1e-6);
    }
}

// --- CV ---------------------------------------------------------------------------------

fn transform_eff_after(h: &mut DeviceHarness, batches: usize) -> f32 {
    let x = vec![0.0f32; BATCH];
    for _ in 0..batches {
        h.input_mut("in_l").copy_from_slice(&x);
        h.process();
    }
    meters(h)[2]
}

#[test]
fn the_transform_cv_adds_to_the_knob_scaled_by_its_trim_and_clamps() {
    for (cv, trim, want) in [
        (1.0f32, 1.0f32, 1.0f32),
        (-1.0, 1.0, 0.0),
        (0.2, 1.0, 0.7),
        (0.2, 0.5, 0.6),
        (-0.3, 1.0, 0.2),
        (1.0, 0.0, 0.5),
        (-1.0, 0.0, 0.5),
    ] {
        let mut h = dev();
        h.set_param("transform", 0.5);
        h.set_param("transform_cv_trim", trim);
        h.input_mut("transform_cv_in").fill(cv);
        let got = transform_eff_after(&mut h, 300);
        assert!(
            (got - want).abs() < 1e-4,
            "cv {cv} trim {trim}: {got} vs {want}"
        );
    }
    // Unconnected: the knob stands.
    let mut h = dev();
    h.set_param("transform", 0.37);
    assert!((transform_eff_after(&mut h, 300) - 0.37).abs() < 1e-5);
}

#[test]
fn a_step_on_the_transform_cv_reaches_90_percent_in_5_to_15_ms() {
    let mut h = dev();
    h.set_param("transform", 0.0);
    h.input_mut("transform_cv_in").fill(0.0);
    let _ = transform_eff_after(&mut h, 20);
    h.input_mut("transform_cv_in").fill(1.0);
    let mut reached = None;
    for b in 1..40 {
        h.input_mut("in_l").fill(0.0);
        h.process();
        if reached.is_none() && meters(&h)[2] >= 0.9 {
            reached = Some(b as f64 * BATCH as f64 / SR * 1000.0);
        }
    }
    let ms = reached.expect("reaches 90 %");
    assert!((5.0..=15.0).contains(&ms), "{ms} ms");
}

#[test]
fn the_mix_cv_law_trim_and_step_response() {
    // Preset 0 of the test chains inverts: out = (1 - 2 m) x, so m is observable.
    let observe = |mix: f32, trim: f32, cv: f32, batches: usize| {
        let mut h = dev_with_test_chains();
        h.set_param("mix", mix);
        h.set_param("mix_cv_trim", trim);
        h.input_mut("mix_cv_in").fill(cv);
        let x = vec![0.5f32; BATCH * batches];
        let (l, _) = run(&mut h, &x, None);
        let v = l[l.len() - 1];
        (1.0 - v / 0.5) / 2.0
    };
    assert!((observe(0.5, 1.0, 1.0, 100) - 1.0).abs() < 1e-5);
    assert!((observe(0.5, 1.0, -1.0, 100)).abs() < 1e-5);
    assert!((observe(0.5, 0.0, 1.0, 100) - 0.5).abs() < 1e-5);
    assert!((observe(0.2, 0.5, 0.4, 100) - 0.4).abs() < 1e-5);
    assert!(
        (observe(0.5, 1.0, 0.0, 100) - 0.5).abs() < 1e-5,
        "zero CV: the knob"
    );
    // Step: from 0 to full within 5 to 15 ms.
    let mut h = dev_with_test_chains();
    h.set_param("mix", 0.0);
    let lat = latency(&h);
    let x = vec![0.5f32; BATCH * 3];
    let _ = run(&mut h, &x, None);
    h.input_mut("mix_cv_in").fill(1.0);
    let x = vec![0.5f32; BATCH * 60];
    let (l, _) = run(&mut h, &x, None);
    let _ = lat;
    let at = l.iter().position(|v| (1.0 - v / 0.5) / 2.0 >= 0.9);
    let ms = at.expect("reaches 90 %") as f64 / SR * 1000.0;
    assert!((5.0..=15.0).contains(&ms), "{ms} ms");
}

// --- preset switching ------------------------------------------------------------------

fn second_diff_max(x: &[f32]) -> f32 {
    x.windows(3)
        .fold(0.0f32, |m, w| m.max((w[2] - 2.0 * w[1] + w[0]).abs()))
}

#[test]
fn all_240_ordered_preset_switches_are_click_free() {
    let n_settle = BATCH * 300;
    let input = sine(BATCH * 700, 1000.0, 0.5, SR);
    let around = |start: usize| start..start + (0.060 * SR) as usize;
    // Steady-state reference per preset on the same input.
    let steady: Vec<f32> = (0..16)
        .map(|p| {
            let mut h = dev();
            set_preset(&mut h, p);
            h.set_param("transform", 1.0);
            let (l, _) = run(&mut h, &input, None);
            second_diff_max(&l[n_settle..n_settle + around(0).len()])
        })
        .collect();
    let mut worst = (0.0f64, 0usize, 0usize);
    for a in 0..16 {
        for b in 0..16 {
            if a == b {
                continue;
            }
            let mut h = dev();
            set_preset(&mut h, a);
            h.set_param("transform", 1.0);
            let (mut l, _) = run(&mut h, &input[..n_settle], None);
            set_preset(&mut h, b);
            let (l2, _) = run(&mut h, &input[n_settle..], None);
            l.extend(l2);
            let win = &l[n_settle - 8..n_settle + around(0).len()];
            let bound = steady[a].max(steady[b]) * 2.0 + 1e-4; // within 6 dB
            let got = second_diff_max(win);
            assert!(got <= bound, "{a} -> {b}: {got} > {bound}");
            let used = f64::from(got / bound);
            if used > worst.0 {
                worst = (used, a, b);
            }
        }
    }
    // MARGIN MONITOR (QA). The pair `ion-sweep -> ghost-pipes` (14 -> 15) is the
    // tightest of the 240: the resonator bank rings up from reset inside the fade, and its
    // largest second difference uses about 99 % of the 6 dB bound (the output is deterministic,
    // so the margin cannot flake, but a retuning of `ghost-pipes`, `ion-sweep` or the fade law
    // eats it). The bound above is NOT to be loosened: when this assertion fires, retune the
    // voicing (or lengthen the fade) instead. The ceiling below is the margin measured at the
    // end of the design work plus a hair; `--nocapture` prints the current worst pair.
    eprintln!(
        "preset switch margin: worst pair {} -> {} uses {:.4} of the bound",
        worst.1, worst.2, worst.0
    );
    assert!(
        worst.0 <= WORST_SWITCH_SHARE,
        "the tightest switch {} -> {} uses {:.4} of the bound (ceiling {WORST_SWITCH_SHARE})",
        worst.1,
        worst.2,
        worst.0
    );
}

#[test]
fn the_outgoing_chain_leaves_no_tail_after_the_fade() {
    for a in 0..8 {
        let mut h = dev();
        set_preset(&mut h, a);
        let burst = noise(BATCH * 300, 21, 0.8);
        let _ = run(&mut h, &burst, None);
        set_preset(&mut h, 8); // a placeholder (silent on silence)
        let silence = vec![0.0f32; BATCH * 250];
        let (l, r) = run(&mut h, &silence, None);
        let late = &l[(0.130 * SR) as usize..];
        let late_r = &r[(0.130 * SR) as usize..];
        assert!(
            db(f64::from(peak(late).max(peak(late_r)))) < -90.0,
            "preset {a}"
        );
    }
}

#[test]
fn a_burst_of_eight_changes_ends_on_the_last_request_within_60_ms() {
    // Test chains: 0 inverts, 1 passes, 8 and 9 are the long class, the rest are placeholders.
    let mut h = dev_with_test_chains();
    h.set_param("mix", 1.0);
    let x = vec![0.25f32; BATCH];
    for p in [8usize, 2, 3, 1, 9, 2, 4, 0] {
        set_preset(&mut h, p);
        for _ in 0..2 {
            h.input_mut("in_l").copy_from_slice(&x);
            h.process();
        }
    }
    let mut last_out = 0.0;
    for _ in 0..(0.060 * SR / BATCH as f64).ceil() as usize + 1 {
        h.input_mut("in_l").copy_from_slice(&x);
        h.process();
        last_out = h.output("out_l")[BATCH - 1];
    }
    // The last request (the inverting chain) is playing: the settled output is minus the input.
    assert!((last_out + 0.25).abs() < 1e-4, "{last_out}");
    assert_eq!(h.device().latency_frames(), short_latency(48_000.0));
}

#[test]
fn a_switch_while_the_transport_is_silent_is_silent() {
    let mut h = dev();
    let z = vec![0.0f32; BATCH * 100];
    for p in 0..16 {
        set_preset(&mut h, p);
        let (l, r) = run(&mut h, &z, None);
        assert!(
            l.iter()
                .chain(r.iter())
                .all(|v| v.abs() < f32::MIN_POSITIVE),
            "preset {p}"
        );
    }
}

// --- determinism, noise and the presence gate -----------------------------------

fn render_preset(h: &mut DeviceHarness, preset: usize, x: &[f32]) -> Vec<f32> {
    set_preset(h, preset);
    run(h, x, None).0
}

#[test]
fn rendering_is_deterministic_seeded_and_restartable() {
    let x = noise(BATCH * 400, 31, 0.5);
    for preset in 0..16 {
        let a = render_preset(&mut dev(), preset, &x);
        let b = render_preset(&mut dev(), preset, &x);
        assert_eq!(a, b, "preset {preset}");
        // Reset restores the start state, for the wobble presets too (`Wobble::restart`): a
        // second render after a reset equals the first.
        let mut h = dev();
        let first = render_preset(&mut h, preset, &x);
        h.device_mut().reset();
        let again = run(&mut h, &x, None).0;
        assert_eq!(first, again, "reset, preset {preset}");
    }
    // Different seeds differ for every preset that draws random numbers.
    for preset in [0usize, 5, 6, 7, 8, 10, 11, 12, 13, 15] {
        let mut h2 = DeviceHarness::with_options(Box::new(Character::new()), SR, 120.0, 12);
        let a = render_preset(&mut dev(), preset, &x);
        let b = render_preset(&mut h2, preset, &x);
        assert_ne!(a, b, "preset {preset} noise depends on the seed");
    }
    // A prepare at another rate and back reproduces the first render.
    let mut h = dev();
    let first = render_preset(&mut h, 7, &x);
    let mut d = Character::new();
    use atomik_device_sdk::{Device, PrepareInfo};
    let info = |sr: f64| PrepareInfo {
        sample_rate: atomik_core::SampleRate::new(sr).unwrap(),
        seed: 11,
    };
    d.prepare(&info(96_000.0));
    d.prepare(&info(48_000.0));
    let mut h2 = DeviceHarness::with_options(Box::new(d), SR, 120.0, 11);
    assert_eq!(first, render_preset(&mut h2, 7, &x));
}

#[test]
fn instances_on_several_threads_render_identically() {
    let x = noise(BATCH * 200, 41, 0.5);
    let reference = render_preset(&mut dev(), 7, &x);
    for workers in [1usize, 2, 4] {
        let outs: Vec<Vec<f32>> = std::thread::scope(|s| {
            (0..workers)
                .map(|_| s.spawn(|| render_preset(&mut dev(), 7, &x)))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|j| j.join().unwrap())
                .collect()
        });
        for o in outs {
            assert_eq!(o, reference, "{workers} workers");
        }
    }
}

#[test]
fn generated_noise_is_gated_by_the_programme_presence() {
    for preset in [0usize, 5, 6, 7, 12, 13] {
        // No input at all: silent.
        let mut h = dev();
        set_preset(&mut h, preset);
        let z = vec![0.0f32; BATCH * 400];
        let (l, r) = run(&mut h, &z, None);
        assert!(
            l.iter()
                .chain(r.iter())
                .all(|v| v.abs() < f32::MIN_POSITIVE),
            "idle preset {preset}"
        );
        // Programme (transform at 1 so the noise is loud), then silence.
        let mut h = dev();
        set_preset(&mut h, preset);
        h.set_param("transform", 1.0);
        h.set_param("mix", 1.0);
        let prog = noise(BATCH * 750, 51, 0.3);
        let mut heard = 0.0f32;
        for _ in 0..13 {
            let (l, _) = run(&mut h, &prog, None);
            heard = heard.max(peak(&l));
        }
        assert!(heard > 0.0);
        // After the hold and the fade (<= 3 s + 2 s in the table) the noise is gone.
        let silence = vec![0.0f32; BATCH * 750];
        let mut last = Vec::new();
        for _ in 0..(8.0 * SR / (BATCH * 750) as f64) as usize + 1 {
            last = run(&mut h, &silence, None).0;
        }
        assert!(
            db(f64::from(peak(&last))) < -80.0,
            "preset {preset}: {}",
            db(f64::from(peak(&last)))
        );
    }
}

#[test]
fn noise_sounds_at_the_bottom_of_gain_unless_the_table_says_it_follows() {
    let level_at_bottom = |follows: bool| {
        let csv = if follows {
            VOICING_CSV.replace(
                "dusty-groove,chain,noise_follows_gain,0,0,0",
                "dusty-groove,chain,noise_follows_gain,1,1,1",
            )
        } else {
            VOICING_CSV.to_string()
        };
        assert_eq!(csv != VOICING_CSV, follows, "the replacement hit a row");
        let v = load_voicing(&csv, ROUTES_CSV).unwrap();
        let mut h =
            DeviceHarness::with_options(Box::new(Character::with_voicing(v)), SR, 120.0, 11);
        set_preset(&mut h, 7);
        h.set_param("transform", 1.0);
        h.set_param("gain", 0.0);
        let prog = noise(BATCH * 750, 61, 0.3);
        let mut out = Vec::new();
        for _ in 0..3 {
            out = run(&mut h, &prog, None).0;
        }
        rms(&out)
    };
    assert!(
        level_at_bottom(false) > 1e-4,
        "noise independent of Input Gain"
    );
    assert!(
        level_at_bottom(true) < 1e-9,
        "silent when it follows the gain"
    );
}

#[test]
fn dry_wet_zero_is_silent_of_noise() {
    let mut h = dev();
    set_preset(&mut h, 7);
    h.set_param("transform", 1.0);
    h.set_param("mix", 0.0);
    let prog = noise(BATCH * 750, 71, 0.3);
    let mut out = Vec::new();
    for _ in 0..3 {
        out = run(&mut h, &prog, None).0;
    }
    let lat = latency(&h);
    for n in lat..out.len() {
        assert_eq!(out[n].to_bits(), prog[n - lat].to_bits());
    }
}

// --- Meters ------------------------------------------------------------------------------------

#[test]
fn meter_values_report_the_peak_after_gain_the_output_peak_and_the_effective_transform() {
    let mut h = dev();
    set_preset(&mut h, 2);
    h.set_param("gain", 1.0); // +12 dB
    h.set_param("transform", 0.25);
    h.set_param("volume", 0.5);
    let x = sine(BATCH * 40, 1000.0, 0.1, SR);
    let y = sine(BATCH * 40, 1000.0, 0.05, SR);
    let _ = run(&mut h, &x, Some(&y));
    let m = meters(&h);
    assert!(
        (m[0] - 0.4).abs() < 0.01,
        "in peak after gain, larger channel: {}",
        m[0]
    );
    assert!(m[1] > 0.0 && m[1] < 4.0);
    assert!((m[2] - 0.25).abs() < 1e-5);
    assert!(m[3] >= 0.0);
    // The peak has instant attack: a single loud sample shows at once.
    let mut z = vec![0.0f32; BATCH];
    z[3] = 0.9;
    h.input_mut("in_l").copy_from_slice(&z);
    h.process();
    assert!(meters(&h)[0] > 3.0);
    let mut too_short = [0.0f32; 2];
    assert_eq!(h.device().meter_values(&mut too_short), 2);
}

// --- The tables --------------------------------------------------------------------------------

#[test]
fn the_embedded_tables_are_complete_and_every_rack_program_compiles() {
    let chains: Vec<Box<dyn PresetChain>> = (0..16).map(build_chain).collect();
    let specs: Vec<ChainSpec> = chains.iter().map(|c| c.spec()).collect();
    let v = Voicing::load(VOICING_CSV, ROUTES_CSV, &specs).unwrap();
    for (i, chain_i) in chains.iter().enumerate() {
        let p = v.preset(i);
        assert_eq!(p.has_rows(), i < chain::IMPLEMENTED_PRESETS, "preset {i}");
        assert!(p.morph_count() <= 8, "preset {i}");
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            assert_eq!(chain_i.check(p, sr), Ok(()), "preset {i} sr {sr}");
        }
    }
    // The rack presets use every stage they declare.
    for (i, def) in all_rack_defs() {
        let mut c = rack::RackChain::new(def);
        c.prepare(48_000.0, 1, v.preset(i));
        assert_eq!(c.execution_order().len(), def.stages.len(), "preset {i}");
    }
}

// --- level match and analytic responses ----------------------------------------

#[test]
fn every_voiced_preset_is_level_matched_at_the_middle_and_bounded_at_the_ends() {
    let x = pink(48_000 * 11 / BATCH * BATCH, 99, 0.1);
    for preset in 0..chain::IMPLEMENTED_PRESETS {
        for (t, tol) in [(0.5f32, 1.5f64), (0.0, 6.0), (1.0, 6.0)] {
            let mut h = dev();
            set_preset(&mut h, preset);
            h.set_param("transform", t);
            let (l, r) = run(&mut h, &x, None);
            let g = db(rms(&l[48_000..]).max(rms(&r[48_000..]))) - db(rms(&x[48_000..]));
            assert!(g.abs() <= tol, "preset {preset} T {t}: {g:.2} dB");
            assert!(peak(&l).max(peak(&r)) <= 8.0);
        }
    }
}

/// Amplitude of the component at `hz` (a lock-in over `x`).
fn tone_amp(x: &[f32], hz: f64) -> f64 {
    let (mut c, mut s) = (0.0f64, 0.0f64);
    for (n, v) in x.iter().enumerate() {
        let ph = std::f64::consts::TAU * hz * n as f64 / SR;
        c += f64::from(*v) * ph.cos();
        s += f64::from(*v) * ph.sin();
    }
    2.0 * (c * c + s * s).sqrt() / x.len() as f64
}

/// Steady-state gain in dB of the device at `hz` for a `-50 dBFS` sine (below every threshold);
/// a lock-in measurement, so generated noise does not disturb it.
fn gain_db_at(preset: usize, transform: f32, hz: f64) -> f64 {
    let mut h = dev();
    set_preset(&mut h, preset);
    h.set_param("transform", transform);
    let n = 48_000 / BATCH * BATCH;
    let x = sine(n, hz, 0.003_162_3, SR);
    let (l, _) = run(&mut h, &x, None);
    let lat = latency(&h);
    // Align (the lock-in is phase blind, but the window must hold the settled part only).
    let _ = lat;
    db(tone_amp(&l[n / 2..], hz)) - db(tone_amp(&x[n / 2..], hz))
}

fn table_value(preset: usize, stage: &str, param: &str, t: f32) -> f64 {
    let chains: Vec<Box<dyn PresetChain>> = (0..16).map(build_chain).collect();
    let specs: Vec<ChainSpec> = chains.iter().map(|c| c.spec()).collect();
    let v = Voicing::load(VOICING_CSV, ROUTES_CSV, &specs).unwrap();
    let mut m = Morph::default();
    v.preset(preset).resolve(t, &mut m);
    f64::from(v.preset(preset).val(stage, param).at(&m))
}

fn biquad_db(shape: atomik_dsp::BiquadShape, f: f64, q: f64, g: f64, at: f64) -> f64 {
    atomik_dsp::BiquadCoeffs::design(shape, f, q, g, SR).response_db(at, SR)
}

#[test]
fn loudness_follows_its_three_filter_contour() {
    use atomik_dsp::BiquadShape::{HighShelf, LowShelf, Peaking};
    for t in [0.0f32, 1.0] {
        let tv = |s: &str, p: &str| table_value(1, s, p, t);
        let makeup = tv("comp", "makeup_db");
        let target = tv("chain", "target_gain_db");
        for hz in [60.0, 200.0, 1600.0, 5000.0, 12_000.0] {
            let want = biquad_db(
                LowShelf,
                tv("low", "freq_hz"),
                0.707,
                tv("low", "gain_db"),
                hz,
            ) + biquad_db(
                Peaking,
                tv("mid", "freq_hz"),
                tv("mid", "q"),
                tv("mid", "gain_db"),
                hz,
            ) + biquad_db(
                HighShelf,
                tv("high", "freq_hz"),
                0.707,
                tv("high", "gain_db"),
                hz,
            ) + makeup
                + target;
            let got = gain_db_at(1, t, hz);
            assert!(
                (got - want).abs() < 0.5,
                "T {t} {hz} Hz: {got:.2} vs {want:.2}"
            );
        }
    }
}

#[test]
fn sparkle_follows_its_highpass_and_shelf_at_transform_zero() {
    use atomik_dsp::BiquadShape::{HighShelf, Highpass};
    let tv = |s: &str, p: &str| table_value(2, s, p, 0.0);
    let target = tv("chain", "target_gain_db");
    for hz in [40.0, 100.0, 1000.0, 8000.0] {
        let want = biquad_db(Highpass, tv("hp", "freq_hz"), tv("hp", "q"), 0.0, hz)
            + biquad_db(
                HighShelf,
                tv("shelf", "freq_hz"),
                0.707,
                tv("shelf", "gain_db"),
                hz,
            )
            + target;
        let got = gain_db_at(2, 0.0, hz);
        assert!((got - want).abs() < 0.5, "{hz} Hz: {got:.2} vs {want:.2}");
    }
}

#[test]
fn pocket_radio_has_its_band_edges_where_the_table_puts_them() {
    use atomik_dsp::BiquadShape::{Highpass, Lowpass, Peaking};
    for t in [0.0f32, 1.0] {
        let tv = |s: &str, p: &str| table_value(5, s, p, t);
        let target = tv("chain", "target_gain_db");
        for hz in [150.0, 400.0, 1500.0, 3000.0, 7000.0] {
            let want = biquad_db(Highpass, tv("hp", "freq_hz"), tv("hp", "q"), 0.0, hz)
                + biquad_db(
                    Peaking,
                    tv("pk", "freq_hz"),
                    tv("pk", "q"),
                    tv("pk", "gain_db"),
                    hz,
                )
                + biquad_db(Lowpass, tv("lp", "freq_hz"), tv("lp", "q"), 0.0, hz)
                + biquad_db(Lowpass, tv("bl", "freq_hz"), tv("bl", "q"), 0.0, hz)
                + adaa_droop_db(hz)
                + tv("sat", "makeup_db")
                + tv("comp", "makeup_db")
                + target
                + sat_small_signal_db(tv("sat", "drive_db"));
            let got = gain_db_at(5, t, hz);
            assert!(
                (got - want).abs() < 0.7,
                "T {t} {hz} Hz: {got:.2} vs {want:.2}"
            );
        }
    }
}

/// The droop of the saturator's first-order antiderivative anti-aliasing (a two-tap average at
/// the 2x rate of the oversampled region): `cos(pi f / (2 fs))`.
fn adaa_droop_db(hz: f64) -> f64 {
    db((std::f64::consts::PI * hz / (2.0 * SR)).cos())
}

/// Small-signal gain of the soft clip with partial make-up: `g / sqrt(g)` for drive `g >= 1`.
fn sat_small_signal_db(drive_db: f64) -> f64 {
    if drive_db >= 0.0 {
        0.5 * drive_db
    } else {
        drive_db
    }
}

#[test]
fn reel_rolls_off_its_top_at_the_table_corner() {
    use atomik_dsp::BiquadShape::Lowpass;
    // At Transform 0 the wobble depth is 0 (a pure delay), so the response is the chain's own.
    let t = 0.0f32;
    let tv = |s: &str, p: &str| table_value(0, s, p, t);
    for hz in [4_000.0, 12_000.0] {
        let want = biquad_db(Lowpass, tv("lp", "freq_hz"), tv("lp", "q"), 0.0, hz)
            - biquad_db(Lowpass, tv("lp", "freq_hz"), tv("lp", "q"), 0.0, 1_000.0)
            + adaa_droop_db(hz)
            - adaa_droop_db(1_000.0);
        let got = gain_db_at(0, t, hz) - gain_db_at(0, t, 1_000.0);
        assert!((got - want).abs() < 0.5, "{hz} Hz: {got:.2} vs {want:.2}");
    }
    // The corner falls with Transform (the table's morph).
    assert!(
        table_value(0, "lp", "freq_hz", 1.0) < table_value(0, "lp", "freq_hz", 0.0),
        "the top rolls off further at Transform 1"
    );
    assert!(gain_db_at(0, 1.0, 1_000.0) - gain_db_at(0, 0.0, 1_000.0) < 20.0);
}

#[test]
fn low_end_has_a_flat_main_path_at_zero_and_a_bump_that_rises_with_transform() {
    use atomik_dsp::BiquadShape::{Highpass, Peaking};
    // Transform 0: no parallel branch (its send is closed) and no bump gain: the main path is
    // the 25 Hz high-pass only.
    let tv0 = |s: &str, p: &str| table_value(3, s, p, 0.0);
    let target = tv0("chain", "target_gain_db");
    for hz in [30.0, 60.0, 500.0] {
        let want = biquad_db(Highpass, tv0("hp", "freq_hz"), tv0("hp", "q"), 0.0, hz)
            + biquad_db(
                Peaking,
                tv0("bump", "freq_hz"),
                tv0("bump", "q"),
                tv0("bump", "gain_db"),
                hz,
            )
            + target;
        let got = gain_db_at(3, 0.0, hz);
        assert!((got - want).abs() < 0.5, "{hz} Hz: {got:.2} vs {want:.2}");
    }
    // Transform 1: the bump (plus the parallel branch) lifts its region above the table's
    // main-path expectation.
    let tv1 = |s: &str, p: &str| table_value(3, s, p, 1.0);
    let f = tv1("bump", "freq_hz");
    let main = biquad_db(Peaking, f, tv1("bump", "q"), tv1("bump", "gain_db"), f)
        - biquad_db(Peaking, f, tv1("bump", "q"), tv1("bump", "gain_db"), 1000.0);
    let got = gain_db_at(3, 1.0, f) - gain_db_at(3, 1.0, 1000.0);
    assert!(got > main - 1.0 && main > 3.0, "{got:.2} vs {main:.2}");
}

/// Prints the level of every voiced preset at Transform 0, 0.5 and 1 (pink noise at -20 dBFS
/// RMS) and the worst preset-switch second difference ratio; run with
/// `cargo test -p atomik-devices calibration_report -- --ignored --nocapture` when tuning the
/// voicing table.
#[test]
#[ignore = "a tuning aid, not a check"]
fn calibration_report() {
    let x = pink(48_000 * 11 / BATCH * BATCH, 99, 0.1);
    for preset in 0..chain::IMPLEMENTED_PRESETS {
        let mut row = format!("preset {preset}:");
        for t in [0.0f32, 0.5, 1.0] {
            let mut h = dev();
            set_preset(&mut h, preset);
            h.set_param("transform", t);
            let (l, r) = run(&mut h, &x, None);
            let g = db(rms(&l[48_000..]).max(rms(&r[48_000..]))) - db(rms(&x[48_000..]));
            row.push_str(&format!(
                "  T{t}: {g:+.2} dB (peak {:.2})",
                peak(&l).max(peak(&r))
            ));
        }
        println!("{row}");
    }
    let n_settle = BATCH * 300;
    let input = sine(BATCH * 700, 1000.0, 0.5, SR);
    let win = (0.060 * SR) as usize;
    let steady: Vec<f32> = (0..16)
        .map(|p| {
            let mut h = dev();
            set_preset(&mut h, p);
            h.set_param("transform", 1.0);
            let (l, _) = run(&mut h, &input, None);
            second_diff_max(&l[n_settle..n_settle + win])
        })
        .collect();
    let mut worst = (0.0f32, 0, 0);
    for a in 0..16 {
        for b in 0..16 {
            if a == b {
                continue;
            }
            let mut h = dev();
            set_preset(&mut h, a);
            h.set_param("transform", 1.0);
            let (mut l, _) = run(&mut h, &input[..n_settle], None);
            set_preset(&mut h, b);
            l.extend(run(&mut h, &input[n_settle..], None).0);
            let ratio =
                second_diff_max(&l[n_settle - 8..n_settle + win]) / steady[a].max(steady[b]);
            if ratio > worst.0 {
                worst = (ratio, a, b);
            }
        }
    }
    println!("steady {steady:?}");
    println!(
        "worst switch ratio {:.2} ({} -> {})",
        worst.0, worst.1, worst.2
    );
}

// --- aliasing ----------------------------------------------------------------------------

/// The largest spectral component (dBFS) away from the harmonics of `bin_f` and its image
/// frequencies, for a signal analysed over `n` samples at `sr`.
fn worst_alias_dbfs(y: &[f32], sr: f64, f0: f64) -> f64 {
    let n = y.len();
    let spec = atomik_testkit::signal::magnitude_spectrum(y);
    let scale = n as f64 / 4.0; // a full-scale sine under a Hann window
    let bin = |f: f64| f * n as f64 / sr;
    let mut worst = f64::NEG_INFINITY;
    for (k, m) in spec.iter().enumerate().skip(4) {
        let near_harmonic = (1..=((sr / 2.0 / f0) as usize + 1)).any(|h| {
            let hb = bin(f0 * h as f64);
            (k as f64 - hb).abs() < 6.0
        });
        if !near_harmonic {
            worst = worst.max(db(m / scale));
        }
    }
    worst
}

/// The worst alias level (dBFS) of `preset` at Transform 1, Input Gain +12 dB and a -3 dBFS sine
/// near `f` Hz at `sr`, with the Volume knob at `volume`. The sine is bin-centred for the
/// analysis window (16384 samples, no leakage into the floor).
fn alias_level(preset: usize, sr: f64, f: f64, volume: f32) -> f64 {
    let n = 1 << 15;
    let m = (n / 2) as f64;
    let f0 = (f * m / sr).round() * sr / m;
    let x = sine(n, f0, 0.707, sr);
    let mut h = DeviceHarness::with_options(
        Box::new(Character::with_voicing(quiet_voicing())),
        sr,
        120.0,
        11,
    );
    set_preset(&mut h, preset);
    h.set_param("transform", 1.0);
    h.set_param("gain", 1.0); // +12 dB
    h.set_param("volume", volume);
    let (l, _) = run(&mut h, &x, None);
    let lat = latency(&h);
    worst_alias_dbfs(&l[lat + 4096..lat + 4096 + n / 2], sr, f0)
}

#[test]
fn the_saturating_presets_meet_the_alias_bound_at_transform_one_and_plus_12_db() {
    // 5 kHz and 18 kHz at -3 dBFS leave every non-harmonic component below -70 dBFS at
    // 44.1 and 48 kHz (the oversampler runs 2x; the saturation curves use first-order
    // antiderivative anti-aliasing) and below -60 dBFS at 96 kHz (no oversampling). Volume
    // is at -24 dB: the output safety beyond +-4.0 is a nonlinearity of its own (see the next
    // test), and Volume is the user's compensation for a high Input Gain.
    let mut worst = (f64::MIN, 0, 0.0, 0.0);
    for sr in [44_100.0f64, 48_000.0, 96_000.0] {
        for preset in [0usize, 2, 3, 5, 10] {
            for f in [5_000.0f64, 18_000.0] {
                let a = alias_level(preset, sr, f, 0.125);
                let bound = if sr > 60_000.0 { -60.0 } else { -70.0 };
                assert!(
                    a <= bound,
                    "preset {preset} at {sr} Hz, {f} Hz: {a:.1} dBFS above {bound}"
                );
                if sr < 60_000.0 && a > worst.0 {
                    worst = (a, preset, sr, f);
                }
                // The achieved levels are far below the bounds at 96 kHz.
                if sr > 60_000.0 {
                    assert!(a <= -80.0, "preset {preset} {f} Hz at 96 kHz: {a:.1}");
                }
            }
        }
    }
    // The worst of the 44.1 and 48 kHz cases is the achieved guard of this test.
    assert!(worst.0 <= -70.5, "{worst:?}");
}

#[test]
fn the_output_safety_is_the_loudest_nonlinearity_at_full_gain_and_default_volume() {
    // The same sparkle test at the default Volume (0 dB) drives the output beyond the +-4.0
    // knee of the safety, whose soft saturation aliases far above the preset's own: the alias
    // bound is a property of the chain, not of the safety, which only bounds the output.
    let with_safety = alias_level(2, 48_000.0, 5_000.0, 0.5);
    let without = alias_level(2, 48_000.0, 5_000.0, 0.125);
    assert!(
        with_safety > without + 25.0,
        "{with_safety} against {without}"
    );
    assert!(with_safety.is_finite() && with_safety < 6.0);
}

// --- no zipper while Transform sweeps -----------------------------------------------------

fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
}

#[test]
fn a_transform_sweep_makes_no_sample_step_beyond_the_steady_state_bound() {
    let n = BATCH * 750; // one second
    let x = sine(n, 440.0, 0.3, SR);
    for preset in 0..chain::IMPLEMENTED_PRESETS {
        // Steady states at nine Transform positions: a swept comb crosses its resonances, so the
        // largest steps are not at the three table points.
        let steady = [0.0f32, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0].map(|t| {
            let mut h = dev();
            set_preset(&mut h, preset);
            h.set_param("transform", t);
            let (l, r) = run(&mut h, &x, None);
            max_step(&l[BATCH * 100..]).max(max_step(&r[BATCH * 100..]))
        });
        // A resonator bank (ghost-pipes) rings at transient peaks that no steady state shows
        // (a swept mode crosses the tone); its limiter bounds the output instead.
        let margin = if preset == 15 { 2.8 } else { 1.6 };
        let bound = steady.iter().fold(0.0f32, |m, v| m.max(*v)) * margin + 1e-3;
        let mut h = dev();
        set_preset(&mut h, preset);
        let mut all_l = Vec::new();
        let mut all_r = Vec::new();
        for (b, chunk) in x.chunks(BATCH).enumerate() {
            h.set_param("transform", b as f32 / 749.0);
            let (l, r) = run(&mut h, chunk, None);
            all_l.extend(l);
            all_r.extend(r);
        }
        let got = max_step(&all_l[BATCH * 100..]).max(max_step(&all_r[BATCH * 100..]));
        assert!(got <= bound, "preset {preset}: step {got} > {bound}");
    }
}

/// The shipped tables with the wobble and the hiss switched off, so that the only non-harmonic
/// components left in a sine test are aliases.
fn quiet_voicing() -> Voicing {
    let csv = VOICING_CSV
        .replace(
            "reel,wobble,depth_ms,0,0.1,0.3",
            "reel,wobble,depth_ms,0,0,0",
        )
        .replace(
            "reel,hiss,level_db,-120,-78,-58",
            "reel,hiss,level_db,-120,-120,-120",
        )
        .replace(
            "pocket-radio,hiss,level_db,-75,-65,-55",
            "pocket-radio,hiss,level_db,-120,-120,-120",
        );
    assert_ne!(csv, VOICING_CSV);
    load_voicing(&csv, ROUTES_CSV).unwrap()
}

// --- State, routing ----------------------------------------------------------------------------

#[test]
fn params_survive_a_prepare_and_an_engine_restart_reproduces_the_render() {
    use atomik_device_sdk::{Device, PrepareInfo};
    let set = |h: &mut DeviceHarness| {
        set_preset(h, 5);
        for (k, v) in [
            ("gain", 0.6),
            ("transform", 0.8),
            ("mix", 0.7),
            ("volume", 0.4),
            ("transform_cv_trim", 0.3),
            ("mix_cv_trim", 0.2),
        ] {
            h.set_param(k, v);
        }
    };
    let x = noise(BATCH * 120, 77, 0.4);
    let mut h = dev();
    set(&mut h);
    let first = run(&mut h, &x, None).0;
    // The same params restored on a re-prepared device give the same bounce.
    let mut d = Character::new();
    d.prepare(&PrepareInfo {
        sample_rate: atomik_core::SampleRate::new(SR).unwrap(),
        seed: 11,
    });
    let mut h2 = DeviceHarness::with_options(Box::new(d), SR, 120.0, 11);
    set(&mut h2);
    assert_eq!(first, run(&mut h2, &x, None).0);
    // Out-of-range and non-finite values are clamped, never stored raw.
    let mut c = Character::new();
    c.set_param(atomik_core::ParamId(1), 99.0);
    c.set_param(atomik_core::ParamId(2), f32::NAN);
    c.set_param(atomik_core::ParamId(99), 1.0);
    assert_eq!(c.active_preset(), 0);
}

#[test]
fn a_mono_sum_preset_sends_the_same_signal_to_both_outputs() {
    let mut h = dev();
    set_preset(&mut h, 5);
    let x = noise(BATCH * 60, 1, 0.5);
    let y = noise(BATCH * 60, 2, 0.5);
    let (l, r) = run(&mut h, &x, Some(&y));
    assert_eq!(l, r);
    assert!(peak(&l) > 0.01);
}

#[test]
fn the_latency_is_known_before_the_first_batch() {
    let mut h = dev();
    assert_eq!(h.device().latency_frames(), short_latency(48_000.0));
    set_preset(&mut h, 9);
    assert_eq!(h.device().latency_frames(), short_latency(48_000.0));
    let h = dev_with_test_chains();
    assert_eq!(h.device().latency_frames(), short_latency(48_000.0));
    let mut h = dev_with_test_chains();
    set_preset(&mut h, 8);
    assert_eq!(
        h.device().latency_frames(),
        short_latency(48_000.0) + EXTRA,
        "a long class preset requested before the first batch is the first latency"
    );
}

mod qa6;
mod row34;
