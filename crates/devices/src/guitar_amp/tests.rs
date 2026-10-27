//! Guitar amp tests. Everything runs through `DeviceHarness`, which guards every `process` call
//! against allocation.

use super::setups::{SETUP_NAMES, SETUP_PARAMS, SETUPS, setup_at};
use super::*;
use crate::test_support::{
    SR, assert_keys, harness, run_signal, sine, sweep_params_min_max, tone_amplitude,
};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::{db, magnitude_spectrum, rms};

const IN: [&str; 2] = ["in_l", "in_r"];
const OUT: [&str; 2] = ["out_l", "out_r"];

fn amp() -> DeviceHarness {
    harness(Box::new(GuitarAmp::new()))
}

fn set(h: &mut DeviceHarness, params: &[(&str, f32)]) {
    for (k, v) in params {
        h.set_param(k, *v);
    }
}

/// Loads setup `i` into the harness (what the app does with one batch of param changes).
fn load(h: &mut DeviceHarness, i: usize) {
    for (k, v) in setup_at(i).values {
        h.set_param(k, v);
    }
    h.set_param("setup", i as f32);
}

/// All effects off, flat-ish, for tests of the amp alone.
fn dry(h: &mut DeviceHarness) {
    set(
        h,
        &[
            ("chorus.on", 0.0),
            ("delay.on", 0.0),
            ("reverb.on", 0.0),
            ("gate", -80.0),
        ],
    );
}

/// `batches` batches of the signal on both inputs; returns left and right.
fn render(
    h: &mut DeviceHarness,
    signal: impl Fn(usize) -> f32,
    batches: usize,
) -> (Vec<f32>, Vec<f32>) {
    let mut o = run_signal(h, &IN, signal, batches, &OUT);
    let r = o.pop().unwrap();
    (o.pop().unwrap(), r)
}

#[test]
fn descriptor_validates() {
    let mut keys: Vec<&str> = vec!["setup"];
    keys.extend(SETUPS[0].values.iter().map(|(k, _)| *k));
    keys.push("bypass");
    assert_keys(&DESCRIPTOR, &keys, &["in_l", "in_r", "out_l", "out_r"]);
    assert_eq!(DESCRIPTOR.type_id, "atomik.guitar-amp");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    assert_eq!(DESCRIPTOR.params.len(), N_PARAMS);
    assert_eq!(SETUP_PARAMS + 2, N_PARAMS);
}

#[test]
fn every_setup_covers_every_param_in_range_and_the_first_is_the_default() {
    assert_eq!(SETUPS.len(), SETUP_NAMES.len());
    for (i, s) in SETUPS.iter().enumerate() {
        assert_eq!(s.name, SETUP_NAMES[i]);
        // Param by param, in order, within the descriptor's range; choices are whole numbers.
        for (n, (key, value)) in s.values.iter().enumerate() {
            let p = &DESCRIPTOR.params[n + 1];
            assert_eq!(*key, p.key, "{} #{n}", s.name);
            assert!(
                (p.min..=p.max).contains(value),
                "{}: {key} = {value} outside {}..{}",
                s.name,
                p.min,
                p.max
            );
            if p.discrete {
                assert_eq!(value.fract(), 0.0, "{}: {key}", s.name);
            }
        }
    }
    for (key, value) in SETUPS[0].values {
        assert_eq!(
            DESCRIPTOR.param_by_key(key).unwrap().default.to_bits(),
            value.to_bits(),
            "the default of {key} is the Clean setup's"
        );
    }
    assert_eq!(DESCRIPTOR.param_by_key("setup").unwrap().default, 0.0);
    // No product names in the setup names.
    for name in SETUP_NAMES {
        assert!(!name.is_empty());
    }
    assert_eq!(setup_at(99).name, "Synthwave");
}

#[test]
fn the_setups_differ_in_the_way_their_names_say() {
    let get = |i: usize, key: &str| {
        setup_at(i)
            .values
            .iter()
            .find(|(k, _)| *k == key)
            .unwrap()
            .1
    };
    let (clean, crunch, industrial, lead, synth) = (0, 1, 2, 3, 4);
    // Gain: clean < crunch < the high-gain setups.
    assert!(get(clean, "gain") < get(crunch, "gain"));
    assert!(get(crunch, "gain") < get(lead, "gain"));
    assert!(get(industrial, "gain") >= 9.0, "very high gain");
    // The industrial rhythm is tight, gated and scooped, with a closed 4x12.
    assert!(get(industrial, "tight") >= 100.0 && get(industrial, "boost") > 0.0);
    assert!(get(industrial, "gate") > -80.0);
    assert!(get(industrial, "mid") < 5.0 && get(industrial, "presence") >= 6.0);
    assert_eq!(get(industrial, "cab"), 2.0);
    assert_eq!(
        get(industrial, "delay.on") + get(industrial, "reverb.on"),
        0.0
    );
    // The lead is mid-forward with a ~380 ms delay and a reverb.
    assert!(get(lead, "mid") > 7.0);
    assert_eq!(get(lead, "delay.time"), 380.0);
    assert_eq!(get(lead, "delay.sync"), 0.0);
    assert_eq!(get(lead, "delay.on"), 1.0);
    assert_eq!(get(lead, "reverb.on"), 1.0);
    // The synthwave setup: bright, chorus, a dotted-eighth delay, a long reverb.
    assert!(get(synth, "treble") > get(synth, "bass"));
    assert_eq!(get(synth, "chorus.on"), 1.0);
    assert_eq!(SYNC_NAMES[get(synth, "delay.sync") as usize], "1/8D");
    assert!(get(synth, "reverb.decay") >= 3.0);
    assert!(get(synth, "gain") < get(industrial, "gain"));
}

#[test]
fn the_latency_is_the_oversampler_round_trip() {
    assert_eq!(GuitarAmp::new().latency_frames(), 35);
    assert_eq!(amp().device().latency_frames(), 35);
    let mut r = atomik_device_sdk::Registry::new();
    crate::register_all(&mut r).unwrap();
    assert_eq!(r.create("atomik.guitar-amp").unwrap().latency_frames(), 35);
}

#[test]
fn the_setup_param_is_a_label_the_device_ignores() {
    let sound = |setup: f32| {
        let mut h = amp();
        dry(&mut h);
        h.set_param("setup", setup);
        render(&mut h, sine(220.0, 0.3), 200).0
    };
    let a = sound(0.0);
    let b = sound(3.0);
    assert!(a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()));
}

#[test]
fn gain_staging_is_clean_at_the_bottom_and_compressed_at_the_top() {
    let level = |gain: f32, amp_in: f64| {
        let mut h = amp();
        dry(&mut h);
        set(&mut h, &[("gain", gain), ("sag", 0.0), ("tight", 20.0)]);
        let (l, _) = render(&mut h, sine(110.0, amp_in), 400);
        rms(&l[l.len() / 2..])
    };
    // Gain 0: a 6 dB louder input comes out 6 dB louder (a linear amp).
    let lin = db(level(0.0, 0.02) / level(0.0, 0.01));
    assert!((lin - 6.0).abs() < 0.5, "clean {lin}");
    // Gain 10: a 20 dB louder input comes out much less than 20 dB louder.
    let comp = db(level(10.0, 0.2) / level(10.0, 0.02));
    assert!(comp < 6.0, "saturated {comp}");
    // More gain is more level into the saturation.
    assert!(level(8.0, 0.05) > 2.0 * level(1.0, 0.05));
}

#[test]
fn a_high_note_at_high_gain_stays_clear_of_aliasing() {
    // 5859.375 Hz is bin 1000 of 8192: the harmonics below Nyquist fall on bins, what is left
    // between them is what folded back.
    let mut h = amp();
    dry(&mut h);
    set(
        &mut h,
        &[
            ("gain", 10.0),
            ("cab", 3.0),
            ("master", 8.0),
            ("tight", 20.0),
        ],
    );
    let batches = (8192 + 4096) / BATCH;
    let (l, _) = render(&mut h, sine(5_859.375, 0.5), batches);
    let y = &l[4096..4096 + 8192];
    let m = magnitude_spectrum(y);
    let near = |k: usize| (1..=4).any(|n| k.abs_diff(n * 1000) <= 4);
    let fund: f64 = (996..=1004).map(|k| m[k] * m[k]).sum();
    let alias: f64 = (8..m.len() - 1)
        .filter(|k| !near(*k))
        .map(|k| m[k] * m[k])
        .sum();
    let r = 10.0 * (alias / fund).log10();
    println!("alias {r:.1} dB");
    assert!(r < -40.0, "{r}");
}

#[test]
fn the_gate_closes_on_noise_and_passes_a_note() {
    let noise_floor = |gate: f32| {
        let mut h = amp();
        dry(&mut h);
        set(&mut h, &[("gain", 9.0), ("gate", gate)]);
        let mut rng = Rng::new(5);
        let hiss: Vec<f32> = (0..BATCH * 400)
            .map(|_| rng.next_bipolar() * 0.002)
            .collect();
        let (l, _) = render(&mut h, |n| hiss[n], 400);
        rms(&l[l.len() / 2..])
    };
    let open = noise_floor(-80.0);
    let closed = noise_floor(-50.0);
    assert!(
        db(open / closed) > 25.0,
        "the gate takes the hiss away: {} vs {}",
        db(open),
        db(closed)
    );
    // A note well above the threshold passes at nearly the same level, and the gate reports no
    // reduction while it is open.
    let note = |gate: f32| {
        let mut h = amp();
        dry(&mut h);
        set(&mut h, &[("gain", 9.0), ("gate", gate)]);
        let (l, _) = render(&mut h, sine(196.0, 0.2), 400);
        let mut m = [0.0f32; 3];
        h.device().meter_values(&mut m);
        (rms(&l[l.len() / 2..]), m[2])
    };
    let (open, _) = note(-80.0);
    let (gated, gr) = note(-40.0);
    assert!(db(gated / open).abs() < 0.5, "{}", db(gated / open));
    assert!(gr < 1.0, "{gr}");
}

#[test]
fn the_gate_is_a_wire_when_off() {
    let run = |gate: f32| {
        let mut h = amp();
        dry(&mut h);
        h.set_param("gate", gate);
        render(&mut h, sine(330.0, 0.1), 50).0
    };
    // -80 and -79.95 are both "off": the same bits.
    let (a, b) = (run(-80.0), run(-79.95));
    assert!(a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()));
}

#[test]
fn the_cabinet_filters_and_off_lets_the_top_through() {
    // The level of 8.4 kHz against 1 kHz through a nearly linear amp.
    let top_to_mid = |cab: f32| {
        let level = |hz: f64| {
            let mut h = amp();
            dry(&mut h);
            set(
                &mut h,
                &[
                    ("gain", 0.5),
                    ("cab", cab),
                    ("master", 3.0),
                    ("presence", 0.0),
                ],
            );
            let (l, _) = render(&mut h, sine(hz, 0.02), 400);
            rms(&l[l.len() / 2..])
        };
        db(level(8_437.5) / level(1_000.0))
    };
    let off = top_to_mid(3.0);
    for cab in [0.0, 1.0, 2.0] {
        let c = top_to_mid(cab);
        assert!(off > c + 8.0, "cab {cab}: {off} vs {c}");
    }
}

#[test]
fn the_mic_moves_the_top() {
    let level = |mic: f32| {
        let mut h = amp();
        dry(&mut h);
        set(&mut h, &[("gain", 3.0), ("cab", 2.0), ("mic", mic)]);
        let (l, _) = render(&mut h, sine(3_750.0, 0.1), 400);
        rms(&l[l.len() / 2..])
    };
    assert!(
        level(0.0) > 1.5 * level(1.0),
        "{} {}",
        level(0.0),
        level(1.0)
    );
}

#[test]
fn the_tone_controls_move_their_bands() {
    let level = |key: &str, v: f32, hz: f64| {
        let mut h = amp();
        dry(&mut h);
        set(
            &mut h,
            &[
                ("gain", 3.0),
                ("cab", 3.0),
                ("presence", 0.0),
                ("depth", 0.0),
                ("master", 3.0),
                (key, v),
            ],
        );
        let (l, _) = render(&mut h, sine(hz, 0.05), 400);
        rms(&l[l.len() / 2..])
    };
    assert!(level("bass", 9.0, 100.0) > 1.5 * level("bass", 1.0, 100.0));
    assert!(level("treble", 9.0, 3_000.0) > 1.5 * level("treble", 1.0, 3_000.0));
    assert!(level("mid", 9.0, 500.0) > 1.3 * level("mid", 1.0, 500.0));
}

#[test]
fn level_scales_the_output_in_decibels() {
    let level = |lv: f32| {
        let mut h = amp();
        dry(&mut h);
        set(&mut h, &[("level", lv)]);
        let (l, _) = render(&mut h, sine(220.0, 0.1), 300);
        rms(&l[l.len() / 2..])
    };
    assert!((db(level(6.0) / level(0.0)) - 6.0).abs() < 0.05);
    assert!((db(level(-12.0) / level(0.0)) + 12.0).abs() < 0.05);
}

#[test]
fn a_loud_burst_is_bounded() {
    let mut h = amp();
    set(
        &mut h,
        &[
            ("gain", 10.0),
            ("master", 10.0),
            ("level", 12.0),
            ("input", 24.0),
            ("cab", 3.0),
        ],
    );
    let (l, r) = render(&mut h, |n| if n % 97 < 40 { 1.0 } else { -1.0 }, 300);
    assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() <= 16.0));
}

#[test]
fn the_output_is_dual_mono_without_effects_and_wide_with_a_chorus() {
    let mut h = amp();
    dry(&mut h);
    let (l, r) = render(&mut h, sine(330.0, 0.1), 100);
    assert!(l.iter().zip(&r).all(|(a, b)| a.to_bits() == b.to_bits()));
    let mut h = amp();
    dry(&mut h);
    set(
        &mut h,
        &[
            ("chorus.on", 1.0),
            ("chorus.mix", 0.5),
            ("chorus.depth", 0.8),
        ],
    );
    let (l, r) = render(&mut h, sine(330.0, 0.1), 300);
    let diff: f32 = l[8_000..]
        .iter()
        .zip(&r[8_000..])
        .map(|(a, b)| (a - b).abs())
        .sum();
    assert!(diff > 1.0, "the chorus spreads the image: {diff}");
}

/// The position of the loudest sample of `x` at or after `from`.
fn peak_after(x: &[f32], from: usize) -> usize {
    (from..x.len())
        .max_by(|a, b| x[*a].abs().total_cmp(&x[*b].abs()))
        .unwrap()
}

/// A single click at sample 0 on both inputs.
fn click(n: usize) -> f32 {
    if n < 8 { 0.5 } else { 0.0 }
}

#[test]
fn the_delay_repeats_at_its_time_and_follows_the_tempo() {
    // 380 ms free: the first repeat lands 380 ms after the dry sound (the left line, since the
    // mono sum goes into it first).
    let mut h = amp();
    dry(&mut h);
    set(
        &mut h,
        &[
            ("delay.on", 1.0),
            ("delay.time", 380.0),
            ("delay.feedback", 0.4),
            ("delay.mix", 0.5),
            ("cab", 3.0),
        ],
    );
    let (l, _) = render(&mut h, click, 400);
    let first = peak_after(&l, 9_000);
    let want = 35 + (0.380 * SR) as usize;
    assert!(
        first.abs_diff(want) < 400,
        "first repeat at {first}, expected near {want}"
    );
    // 1/8D at 120 BPM is 375 ms.
    let mut h = amp();
    dry(&mut h);
    set(
        &mut h,
        &[
            ("delay.on", 1.0),
            ("delay.sync", 7.0),
            ("delay.time", 900.0),
            ("delay.mix", 0.5),
            ("cab", 3.0),
        ],
    );
    let (l, _) = render(&mut h, click, 400);
    let first = peak_after(&l, 9_000);
    let want = 35 + (0.375 * SR) as usize;
    assert!(
        first.abs_diff(want) < 400,
        "synced repeat at {first}, expected near {want}"
    );
    // At 60 BPM the same note value takes twice as long.
    let mut h = DeviceHarness::with_options(Box::new(GuitarAmp::new()), SR, 60.0, 0);
    dry(&mut h);
    set(
        &mut h,
        &[
            ("delay.on", 1.0),
            ("delay.sync", 7.0),
            ("delay.mix", 0.5),
            ("cab", 3.0),
        ],
    );
    let (l, _) = render(&mut h, click, 600);
    let first = peak_after(&l, 9_000);
    let want = 35 + (0.750 * SR) as usize;
    assert!(first.abs_diff(want) < 600, "{first} {want}");
}

#[test]
fn a_zero_mix_or_an_off_switch_is_the_dry_signal_bit_for_bit() {
    let run = |params: &[(&str, f32)]| {
        let mut h = amp();
        dry(&mut h);
        set(&mut h, params);
        render(&mut h, sine(261.6, 0.2), 120)
    };
    let (rl, rr) = run(&[]);
    for params in [
        &[("delay.on", 1.0), ("delay.mix", 0.0)][..],
        &[("reverb.on", 1.0), ("reverb.mix", 0.0)][..],
        &[("chorus.on", 1.0), ("chorus.mix", 0.0)][..],
        &[("delay.on", 0.0), ("delay.mix", 1.0)][..],
        &[("reverb.on", 0.0), ("reverb.mix", 1.0)][..],
        &[("chorus.on", 0.0), ("chorus.mix", 1.0)][..],
    ] {
        let (l, r) = run(params);
        assert!(
            l.iter().zip(&rl).all(|(a, b)| a.to_bits() == b.to_bits()),
            "{params:?}"
        );
        assert!(
            r.iter().zip(&rr).all(|(a, b)| a.to_bits() == b.to_bits()),
            "{params:?}"
        );
    }
}

#[test]
fn the_reverb_rings_on_after_the_note_and_decays_with_its_time() {
    let tail = |decay: f32| {
        let mut h = amp();
        dry(&mut h);
        set(
            &mut h,
            &[
                ("reverb.on", 1.0),
                ("reverb.mix", 0.6),
                ("reverb.decay", decay),
                ("cab", 3.0),
            ],
        );
        let (l, _) = render(
            &mut h,
            |n| if n < 4_800 { sine(300.0, 0.2)(n) } else { 0.0 },
            1_000,
        );
        rms(&l[40_000..48_000])
    };
    let (short, long) = (tail(0.4), tail(6.0));
    assert!(long > 3.0 * short, "{short} {long}");
    assert!(long > 1e-4, "there is a tail: {long}");
    // Without it the room is silent that long after the note.
    let mut h = amp();
    dry(&mut h);
    let (l, _) = render(
        &mut h,
        |n| if n < 4_800 { sine(300.0, 0.2)(n) } else { 0.0 },
        1_000,
    );
    assert!(rms(&l[40_000..48_000]) < 1e-6);
}

#[test]
fn silence_after_a_loud_note_decays_to_silence() {
    let mut h = amp();
    load(&mut h, 2);
    let (l, r) = render(
        &mut h,
        |n| if n < 4_800 { sine(82.4, 0.8)(n) } else { 0.0 },
        600,
    );
    assert!(
        l[30_000..]
            .iter()
            .chain(&r[30_000..])
            .all(|v| v.abs() < 1e-5)
    );
}

#[test]
fn one_connected_input_is_the_mono_signal_and_both_are_averaged() {
    let run = |connect_l: bool, connect_r: bool, l_amp: f64, r_amp: f64| {
        let mut h = amp();
        dry(&mut h);
        let mut out = Vec::new();
        for b in 0..100 {
            let mut x = [0.0f32; BATCH];
            let mut y = [0.0f32; BATCH];
            for i in 0..BATCH {
                let n = b * BATCH + i;
                x[i] = sine(330.0, l_amp)(n);
                y[i] = sine(330.0, r_amp)(n);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = y;
            // `input_mut` marks a socket connected.
            h.set_connected("in_l", connect_l);
            h.set_connected("in_r", connect_r);
            h.process();
            out.extend_from_slice(h.output("out_l"));
        }
        out
    };
    let both = run(true, true, 0.1, 0.1);
    let left = run(true, false, 0.1, 0.0);
    let right = run(false, true, 0.0, 0.1);
    assert!(
        both.iter()
            .zip(&left)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    assert!(
        both.iter()
            .zip(&right)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
}

#[test]
fn it_is_deterministic() {
    let run = |sr: f64| {
        let mut h = DeviceHarness::with_options(Box::new(GuitarAmp::new()), sr, 120.0, 11);
        load(&mut h, 4);
        let mut rng = Rng::new(9);
        let noise: Vec<f32> = (0..BATCH * 200).map(|_| rng.next_bipolar() * 0.3).collect();
        render(&mut h, |n| noise[n], 200)
    };
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let (a, b) = (run(sr), run(sr));
        assert!(
            a.0.iter()
                .zip(&b.0)
                .all(|(p, q)| p.to_bits() == q.to_bits()),
            "{sr}"
        );
        assert!(
            a.1.iter()
                .zip(&b.1)
                .all(|(p, q)| p.to_bits() == q.to_bits()),
            "{sr}"
        );
    }
}

#[test]
fn every_param_at_its_extremes_stays_finite_and_bounded() {
    let mut h = amp();
    // Feed a loud signal while sweeping so every extreme is heard.
    for p in DESCRIPTOR.params {
        for v in [p.min, p.max, p.default] {
            h.set_param(p.key, v);
            let (l, r) = render(&mut h, sine(196.0, 0.9), 6);
            assert!(
                l.iter()
                    .chain(&r)
                    .all(|x| x.is_finite() && x.abs() <= OUT_LIMIT),
                "{} = {v}",
                p.key
            );
        }
    }
    let mut h = amp();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn bad_param_values_are_clamped() {
    let mut h = amp();
    for p in DESCRIPTOR.params {
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30] {
            h.set_param(p.key, v);
        }
    }
    let (l, r) = render(&mut h, sine(196.0, 0.5), 20);
    assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    let mut x = amp();
    let s = |n| {
        if n == 7 {
            f32::NAN
        } else if n == 9 {
            f32::INFINITY
        } else {
            0.1
        }
    };
    let (l, r) = render(&mut x, s, 20);
    assert!(l.iter().chain(&r).all(|x| x.is_finite()));
}

#[test]
fn the_meters_follow_the_levels() {
    let mut h = amp();
    dry(&mut h);
    render(&mut h, sine(220.0, 0.25), 100);
    let mut m = [0.0f32; 3];
    assert_eq!(h.device().meter_values(&mut m), 3);
    assert!((m[0] - 0.25).abs() < 0.02, "{}", m[0]);
    assert!(m[1] > 0.01);
    assert_eq!(m[2], 0.0);
    assert!(h.device().has_meter_values());
}

#[test]
fn each_setup_makes_sound_at_a_sensible_level() {
    // A plucked-string-like burst through every setup: audible, bounded and within a few dB.
    let mut levels = Vec::new();
    for (i, name) in SETUP_NAMES.iter().enumerate() {
        let mut h = amp();
        load(&mut h, i);
        let mut rng = Rng::new(3);
        let pluck: Vec<f32> = (0..BATCH * 300)
            .map(|n| {
                let env = (-(n as f32) / 9_000.0).exp();
                env * 0.3 * ((n as f32 * 0.0286).sin() + 0.3 * rng.next_bipolar())
            })
            .collect();
        let (l, _) = render(&mut h, |n| pluck[n], 300);
        assert!(l.iter().all(|v| v.is_finite()));
        let r = rms(&l[2_000..12_000]);
        println!("{name}: {:.1} dBFS", db(r));
        levels.push(db(r));
    }
    for (i, l) in levels.iter().enumerate() {
        assert!(*l > -40.0 && *l < -6.0, "{}: {l}", SETUP_NAMES[i]);
    }
    let (lo, hi) = levels
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), l| (a.min(*l), b.max(*l)));
    // The effects of a setup add to a pluck as it rings, so the early part of one differs more
    // than a whole performance does (the bounce test of the render crate checks that one).
    assert!(hi - lo < 8.0, "{levels:?}");
}

#[test]
fn a_tone_at_a_known_level_comes_through_the_clean_setup_linearly() {
    // The clean setup is a clean amp at modest input: harmonics well below the fundamental.
    let mut h = amp();
    load(&mut h, 0);
    set(&mut h, &[("cab", 3.0)]);
    let n = 8192usize;
    let hz = 43.0 * SR / n as f64; // whole cycles in the analysis window
    let (l, _) = render(&mut h, sine(hz, 0.02), (2 * n) / BATCH);
    let y = &l[n..];
    let f = tone_amplitude(y, hz, n);
    let h3 = tone_amplitude(y, 3.0 * hz, n);
    assert!(f > 0.01);
    assert!(db((h3 / f) as f32) < -36.0, "{}", db((h3 / f) as f32));
}

#[test]
fn tight_cuts_the_low_end_before_the_gain_and_boost_lifts_the_mids() {
    let level = |params: &[(&str, f32)], hz: f64| {
        let mut h = amp();
        dry(&mut h);
        set(
            &mut h,
            &[
                ("gain", 0.5),
                ("cab", 3.0),
                ("master", 3.0),
                ("presence", 0.0),
                ("depth", 0.0),
            ],
        );
        set(&mut h, params);
        let (l, _) = render(&mut h, sine(hz, 0.01), 400);
        rms(&l[l.len() / 2..])
    };
    let chug =
        |tight: f32| db(level(&[("tight", tight)], 60.0) / level(&[("tight", tight)], 1_000.0));
    assert!(
        chug(300.0) < chug(20.0) - 10.0,
        "{} {}",
        chug(300.0),
        chug(20.0)
    );
    let lift = db(level(&[("boost", 12.0)], 780.0) / level(&[("boost", 0.0)], 780.0));
    assert!(lift > 8.0 && lift < 16.0, "{lift}");
}

#[test]
fn sag_squashes_a_sustained_loud_note() {
    let ends = |sag: f32| {
        let mut h = amp();
        dry(&mut h);
        set(
            &mut h,
            &[("gain", 3.0), ("master", 8.0), ("sag", sag), ("cab", 3.0)],
        );
        let (l, _) = render(&mut h, sine(196.0, 0.3), 600);
        (rms(&l[1_000..3_000]), rms(&l[l.len() - 4_000..]))
    };
    let (s0, s1) = ends(0.0);
    let (g0, g1) = ends(10.0);
    assert!((s1 / s0 - 1.0).abs() < 0.1, "no sag, no squash: {s0} {s1}");
    assert!(g1 < 0.85 * g0, "sag lowers the sustained level: {g0} {g1}");
}
