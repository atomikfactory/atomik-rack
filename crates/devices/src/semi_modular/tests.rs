//! Tests of the semi-modular synth: the descriptor and tables, each oscillator type (pitch and
//! aliasing), the filter slots and their routing, the shaper, the modulation matrix, the play
//! modes, the sequencer's timing, the controllers and CVs, determinism and the allocation guard.

use super::*;
use crate::test_support::{
    SR, assert_bend_range, assert_sustain_pedal, assert_volume_and_expression, harness, off, on,
    sweep_params_min_max,
};
use atomik_core::NoteEvent;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use atomik_testkit::signal::{dominant_frequency, magnitude_spectrum, max_step, peak, rms};

const OUT: [&str; 2] = ["out_l", "out_r"];

fn seconds(s: f64) -> usize {
    (s * SR / BATCH as f64).ceil() as usize
}

fn synth() -> DeviceHarness {
    harness(Box::new(SemiModular::new()))
}

fn render(h: &mut DeviceHarness, secs: f64) -> Vec<f32> {
    h.render(seconds(secs), &OUT).remove(0)
}

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

fn hz(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// RMS of the first difference: weights high harmonics.
fn brightness(x: &[f32]) -> f32 {
    let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
    rms(&d)
}

fn source(key: &str) -> f32 {
    MOD_TABLES.source_by_key(key).unwrap().id as f32 + 1.0
}

fn target(key: &str) -> f32 {
    MOD_TABLES.target_by_key(key).unwrap().id as f32 + 1.0
}

/// Fills modulation slot `n` (0-based).
fn slot(h: &mut DeviceHarness, n: usize, src: &str, tgt: &str, via: Option<&str>, amount: f32) {
    h.set_param(&format!("internal.mod{}_source", n + 1), source(src));
    h.set_param(&format!("internal.mod{}_target", n + 1), target(tgt));
    h.set_param(
        &format!("internal.mod{}_via", n + 1),
        via.map_or(0.0, source),
    );
    h.set_param(&format!("mod{}_amount", n + 1), amount);
}

/// A neutral voice: oscillator 1 alone (a saw), no filter, no shaper, a flat envelope, level 0 dB.
fn plain(h: &mut DeviceHarness) {
    h.set_param("master.level", 0.0);
    h.set_param("master.spread", 0.0);
    h.set_param("wheel.to_cutoff", 0.0);
    h.set_param("osc1.type", 0.0);
    h.set_param("osc1.level", 1.0);
    h.set_param("osc2.level", 0.0);
    h.set_param("osc3.level", 0.0);
    h.set_param("osc2.fine", 0.0);
    h.set_param("filter1.mode", 0.0);
    h.set_param("filter1.env", 0.0);
    h.set_param("env.amp.attack", 0.5);
    h.set_param("env.amp.sustain", 1.0);
    h.set_param("env.amp.release", 5.0);
}

fn seq_synth() -> DeviceHarness {
    let mut h = synth();
    plain(&mut h);
    h.set_param("seq.run", 1.0);
    h
}

fn push(h: &mut DeviceHarness, offset: u16, kind: NoteKind) {
    h.push_note("notes_in", NoteEvent { offset, kind });
}

// -------------------------------------------------------------------------------- descriptor

#[test]
fn descriptor_and_tables_validate() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    assert_eq!(MOD_TABLES.validate(&DESCRIPTOR), Ok(()));
    assert_eq!(DESCRIPTOR.params.len(), PARAM_COUNT);
    assert_eq!(
        DESCRIPTOR.params[P_MOD_FIRST as usize].key,
        "internal.mod1_source"
    );
    for t in &MOD_TARGETS {
        assert!(
            DESCRIPTOR.params.get(t.param.0 as usize).is_some(),
            "{}",
            t.key
        );
    }
    let key = |key: &str| DESCRIPTOR.param_by_key(key).unwrap().id.0;
    assert_eq!(key("osc2.shape"), OSC_BASE + (OSC_FIELDS + F_SHAPE) as u32);
    assert_eq!(
        key("filter3.mix"),
        FILTER_BASE + (2 * FILTER_FIELDS + G_MIX) as u32
    );
    assert_eq!(key("env.mod.release"), ENV_BASE + 11);
    assert_eq!(key("seq.vel.16"), P_STEP_VEL + 15);
    assert_eq!(key("cv.level_amount"), P_CV_LEVEL);
    let mut r = atomik_device_sdk::Registry::new();
    crate::register_all(&mut r).unwrap();
    assert!(r.mod_tables("atomik.semi-modular").is_some());
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
}

#[test]
fn every_param_at_min_and_max_stays_finite() {
    let mut h = synth();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn a_note_sounds_at_the_default_patch() {
    let mut h = synth();
    h.push_note("notes_in", on(0, 48, 0.9));
    let o = render(&mut h, 0.3);
    assert!(rms(&o[2_000..]) > 0.01);
    assert!(o.iter().all(|v| v.is_finite() && v.abs() < 4.0));
}

// -------------------------------------------------------------------------------- oscillators

fn hold(h: &mut DeviceHarness, key: u8, secs: f64) -> Vec<f32> {
    h.push_note("notes_in", on(0, key, 1.0));
    render(h, secs)
}

#[test]
fn the_oscillator_types_play_the_key() {
    // Types whose strongest partial is the fundamental.
    for (kind, name) in [
        (0.0, "saw"),
        (2.0, "triangle"),
        (3.0, "sine"),
        (4.0, "wave"),
    ] {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", kind);
        let o = hold(&mut h, 57, 0.6);
        let f = dominant_frequency(&o[8_000..8_000 + 16_384], SR);
        assert!((f - hz(57)).abs() < 1.0, "{name}: {f}");
    }
}

#[test]
fn octave_coarse_and_fine_tune_a_slot() {
    let freq = |set: &dyn Fn(&mut DeviceHarness)| {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", 3.0);
        set(&mut h);
        let o = hold(&mut h, 57, 0.6);
        dominant_frequency(&o[8_000..8_000 + 16_384], SR)
    };
    let base = freq(&|_| {});
    assert!((freq(&|h| h.set_param("osc1.octave", 1.0)) / base - 2.0).abs() < 0.01);
    assert!((freq(&|h| h.set_param("osc1.coarse", 7.0)) / base - 1.498_3).abs() < 0.01);
    let fine = freq(&|h| h.set_param("osc1.fine", 50.0)) / base;
    assert!((fine - 2f64.powf(0.5 / 12.0)).abs() < 0.002, "{fine}");
}

/// The share of the spectrum's energy that is not at a harmonic of `f0`.
fn off_harmonic_share(x: &[f32], f0: f64) -> f64 {
    let m = magnitude_spectrum(x);
    let bin = SR / x.len() as f64;
    let (mut harmonic, mut other) = (0.0f64, 0.0f64);
    for (k, v) in m.iter().enumerate().skip(2) {
        let f = k as f64 * bin;
        let n = (f / f0).round().max(1.0);
        if (f - n * f0).abs() < 6.0 * bin {
            harmonic += v * v;
        } else {
            other += v * v;
        }
    }
    other / (harmonic + other)
}

#[test]
fn every_band_limited_type_stays_clean_at_a_high_pitch() {
    // Key 102 is about 2.9 kHz: eight harmonics under Nyquist, so folded partials of a naive
    // oscillator would land between them.
    // (type, shape, key, name): a 5 % pulse is under a sample wide at 2.9 kHz, so it is
    // checked an octave and a half lower.
    for (kind, shape, key, name) in [
        (0.0, 0.0, 102, "saw"),
        (1.0, 0.0, 102, "square"),
        (1.0, 1.0, 84, "narrow pulse"),
        (2.0, 0.0, 102, "triangle"),
        (4.0, 0.0, 102, "wave"),
        (5.0, 0.3, 102, "table"),
        (6.0, 1.0, 102, "fm"),
    ] {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", kind);
        h.set_param("osc1.shape", shape);
        // Wave 2 is the ratio 1 of the FM type, so its sidebands fall on harmonics.
        h.set_param("osc1.wave", 2.0);
        let o = hold(&mut h, key, 0.6);
        let share = off_harmonic_share(&o[8_000..8_000 + 16_384], hz(key));
        assert!(share < 0.01, "{name}: {share}");
    }
}

#[test]
fn the_table_type_moves_with_its_shape_and_the_wave_with_its_index() {
    let spectrum = |kind: f32, wave: f32, shape: f32| {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", kind);
        h.set_param("osc1.wave", wave);
        h.set_param("osc1.shape", shape);
        let o = hold(&mut h, 48, 0.3);
        bits(&o[4_000..8_000])
    };
    assert_ne!(spectrum(5.0, 0.0, 0.0), spectrum(5.0, 0.0, 1.0));
    assert_ne!(spectrum(5.0, 0.0, 0.5), spectrum(5.0, 3.0, 0.5));
    assert_ne!(spectrum(4.0, 0.0, 0.0), spectrum(4.0, 9.0, 0.0));
}

#[test]
fn fm_gets_brighter_with_shape_and_noise_is_noise() {
    let play = |kind: f32, shape: f32| {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", kind);
        h.set_param("osc1.shape", shape);
        h.set_param("osc1.wave", 2.0);
        brightness(&hold(&mut h, 48, 0.3)[4_000..])
    };
    assert!(play(6.0, 0.8) > play(6.0, 0.05) * 2.0);
    assert!(play(6.0, 0.0) > 0.0);
    // Noise does not follow the key: no dominant tone, and an energy spread over the spectrum.
    let mut h = synth();
    plain(&mut h);
    h.set_param("osc1.type", 7.0);
    let o = hold(&mut h, 48, 0.3);
    assert!(rms(&o[4_000..]) > 0.05);
    assert!(off_harmonic_share(&o[4_000..4_000 + 8_192], hz(96)) > 0.8);
}

#[test]
fn the_pulse_width_narrows_the_pulse_without_dc() {
    let mut h = synth();
    plain(&mut h);
    h.set_param("osc1.type", 1.0);
    h.set_param("osc1.shape", 0.6);
    let o = hold(&mut h, 40, 0.8);
    let mean = o[8_000..32_000].iter().map(|v| f64::from(*v)).sum::<f64>() / 24_000.0;
    assert!(mean.abs() < 0.02 * f64::from(peak(&o)), "{mean}");
}

#[test]
fn the_three_slots_mix() {
    let level = |a: f32, b: f32, c: f32| {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.level", a);
        h.set_param("osc2.level", b);
        h.set_param("osc3.level", c);
        h.set_param("osc2.type", 3.0);
        h.set_param("osc3.type", 3.0);
        h.set_param("osc2.octave", 1.0);
        h.set_param("osc3.octave", 2.0);
        rms(&hold(&mut h, 48, 0.3)[4_000..])
    };
    let (one, two, three) = (
        level(1.0, 0.0, 0.0),
        level(1.0, 1.0, 0.0),
        level(1.0, 1.0, 1.0),
    );
    assert!(two > one * 1.1 && three > two * 1.05, "{one} {two} {three}");
    assert_eq!(level(0.0, 0.0, 0.0), 0.0);
}

// -------------------------------------------------------------------------------- filters

/// A saw through filter 1 in `mode` at `cutoff`, no envelope or tracking.
fn filtered(mode: f32, cutoff: f32, resonance: f32) -> DeviceHarness {
    let mut h = synth();
    plain(&mut h);
    h.set_param("filter1.mode", mode);
    h.set_param("filter1.cutoff", cutoff);
    h.set_param("filter1.resonance", resonance);
    h.set_param("filter1.track", 0.0);
    h
}

#[test]
fn the_filter_modes_shape_the_spectrum() {
    let measure = |mode: f32, cutoff: f32| {
        let mut h = filtered(mode, cutoff, 0.1);
        let o = hold(&mut h, 48, 0.6);
        let m = magnitude_spectrum(&o[8_000..8_000 + 16_384]);
        let bin = SR / 16_384.0;
        let band = |lo: f64, hi: f64| {
            m.iter()
                .enumerate()
                .filter(|(k, _)| (*k as f64 * bin) >= lo && (*k as f64 * bin) < hi)
                .map(|(_, v)| v * v)
                .sum::<f64>()
        };
        (band(100.0, 600.0), band(4_000.0, 8_000.0))
    };
    let (open_low, open_high) = measure(0.0, 1_000.0);
    let (lp_low, lp_high) = measure(1.0, 1_000.0);
    let (hp_low, hp_high) = measure(3.0, 1_000.0);
    let (bp_low, bp_high) = measure(2.0, 1_000.0);
    let (nt_low, _) = measure(4.0, 1_000.0);
    let (l24_low, l24_high) = measure(5.0, 1_000.0);
    // A low-pass keeps the lows and takes the highs; a high-pass the opposite.
    assert!(lp_low > open_low * 0.5 && lp_high < open_high * 0.1);
    assert!(hp_high > open_high * 0.5 && hp_low < open_low * 0.05);
    assert!(bp_low < open_low * 0.5 && bp_high < open_high * 0.1);
    assert!(nt_low > 0.0);
    // The ladder rolls off faster than the 12 dB filter.
    assert!(l24_high < lp_high * 0.5 && l24_low > open_low * 0.3);
}

#[test]
fn the_comb_slot_rings_at_the_cutoff_and_stays_bounded() {
    let mut h = filtered(6.0, 440.0, 0.9);
    h.set_param("osc1.type", 7.0);
    h.set_param("filter1.mix", 1.0);
    let o = hold(&mut h, 60, 1.0);
    assert!(o.iter().all(|v| v.is_finite() && v.abs() < 4.0));
    let f = dominant_frequency(&o[8_000..8_000 + 32_768], SR);
    // The noise comes out with peaks at multiples of the comb's frequency (a key-tracked 440 Hz).
    assert!(rms(&o[8_000..]) > 0.01 && f > 100.0, "{f}");
}

#[test]
fn the_filter_envelope_key_track_and_cutoff_cv_move_the_cutoff() {
    let bright = |set: &dyn Fn(&mut DeviceHarness), key: u8| {
        let mut h = filtered(1.0, 400.0, 0.1);
        set(&mut h);
        brightness(&hold(&mut h, key, 0.3)[6_000..])
    };
    let base = bright(&|_| {}, 48);
    assert!(
        bright(
            &|h| {
                h.set_param("filter1.env", 4.0);
                h.set_param("env.filter.decay", 5_000.0);
                h.set_param("env.filter.sustain", 1.0);
            },
            48
        ) > base * 2.0
    );
    assert!(
        bright(&|h| h.set_param("filter1.track", 1.0), 72)
            > bright(&|h| h.set_param("filter1.track", 0.0), 72) * 1.5
    );
    let cv = {
        let mut h = filtered(1.0, 400.0, 0.1);
        h.set_connected("cutoff_cv_in", true);
        *h.input_mut("cutoff_cv_in") = [1.0; BATCH];
        brightness(&hold(&mut h, 48, 0.3)[6_000..])
    };
    assert!(cv > base * 2.0, "{cv} vs {base}");
}

#[test]
fn the_routings_wire_the_slots_differently() {
    let play = |routing: f32| {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", 7.0);
        h.set_param("filter1.mode", 1.0);
        h.set_param("filter1.cutoff", 2_000.0);
        h.set_param("filter1.track", 0.0);
        h.set_param("filter2.mode", 3.0);
        h.set_param("filter2.cutoff", 800.0);
        h.set_param("filter2.track", 0.0);
        h.set_param("filter3.mode", 2.0);
        h.set_param("filter3.cutoff", 1_500.0);
        h.set_param("filter3.track", 0.0);
        h.set_param("filter.routing", routing);
        bits(&hold(&mut h, 48, 0.2))
    };
    let (series, parallel, split) = (play(0.0), play(1.0), play(2.0));
    assert_ne!(series, parallel);
    assert_ne!(series, split);
    assert_ne!(parallel, split);
}

#[test]
fn a_slot_that_is_off_passes_and_mix_zero_is_the_dry_signal() {
    let play = |mode: f32, mix: f32| {
        let mut h = filtered(mode, 300.0, 0.4);
        h.set_param("filter1.mix", mix);
        bits(&hold(&mut h, 48, 0.2))
    };
    assert_eq!(play(0.0, 1.0), play(1.0, 0.0));
    assert_eq!(play(0.0, 1.0), play(6.0, 0.0));
    assert_ne!(play(0.0, 1.0), play(1.0, 1.0));
}

// -------------------------------------------------------------------------------- shaper

/// Harmonics 2 to 8 against the fundamental, in amplitude.
fn distortion(x: &[f32], f0: f64) -> f64 {
    let m = magnitude_spectrum(x);
    let bin = SR / x.len() as f64;
    let at = |f: f64| {
        let k = (f / bin).round() as usize;
        m[k.saturating_sub(3)..=(k + 3).min(m.len() - 1)]
            .iter()
            .fold(0.0f64, |a, b| a.max(*b))
    };
    let rest: f64 = (2..=8).map(|n| at(n as f64 * f0).powi(2)).sum();
    rest.sqrt() / at(f0)
}

#[test]
fn the_shaper_adds_harmonics_and_mix_zero_is_dry() {
    let play = |kind: f32, drive: f32, mix: f32| {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", 3.0);
        h.set_param("shaper.type", kind);
        h.set_param("shaper.drive", drive);
        h.set_param("shaper.mix", mix);
        hold(&mut h, 48, 0.5)
    };
    let dry = play(0.0, 0.0, 1.0);
    for kind in [1.0, 2.0, 3.0, 4.0] {
        let wet = play(kind, 24.0, 1.0);
        let (w, d) = (
            distortion(&wet[4_000..4_000 + 16_384], hz(48)),
            distortion(&dry[4_000..4_000 + 16_384], hz(48)),
        );
        assert!(w > d + 0.05, "{kind}: {w} vs {d}");
        assert!(wet.iter().all(|v| v.is_finite() && v.abs() < 4.0));
        assert_eq!(bits(&play(kind, 24.0, 0.0)), bits(&dry), "{kind}");
    }
}

// -------------------------------------------------------------------------------- matrix

#[test]
fn an_unused_matrix_changes_nothing_bit_for_bit() {
    let play = |configure: &dyn Fn(&mut DeviceHarness)| {
        let mut h = synth();
        configure(&mut h);
        h.push_note("notes_in", on(0, 48, 0.8));
        h.push_note("notes_in", on(3, 55, 0.6));
        bits(&render(&mut h, 0.4))
    };
    let base = play(&|_| {});
    let idle = play(&|h| {
        h.set_param("internal.mod1_source", source("lfo1"));
        h.set_param("mod1_amount", 1.0);
        h.set_param("internal.mod2_target", target("pitch"));
        h.set_param("mod2_amount", 1.0);
        slot(h, 2, "lfo1", "pitch", None, 0.0);
    });
    assert_eq!(base, idle);
}

#[test]
fn an_lfo_to_pitch_slot_makes_vibrato_and_via_the_wheel_scales_it() {
    let spread = |wheel: Option<f32>, via: bool| {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", 3.0);
        h.set_param("lfo1.wave", 3.0); // square: +1 then -1
        h.set_param("lfo1.rate", 0.5);
        slot(
            &mut h,
            0,
            "lfo1",
            "pitch",
            via.then_some("mod_wheel"),
            1.0 / 24.0,
        );
        if let Some(value) = wheel {
            push(
                &mut h,
                0,
                NoteKind::Cc {
                    controller: 1,
                    value,
                },
            );
        }
        h.push_note("notes_in", on(1, 57, 1.0));
        let o = render(&mut h, 2.0);
        dominant_frequency(&o[4_800..43_200], SR) - dominant_frequency(&o[52_800..91_200], SR)
    };
    // One semitone each way around 220 Hz: about 25 Hz apart.
    let free = spread(None, false);
    assert!(free > 20.0 && free < 30.0, "{free}");
    assert!(spread(None, true).abs() < 1.0);
    assert!(spread(Some(1.0), true) > 20.0);
}

#[test]
fn envelope_velocity_and_level_slots_move_their_targets() {
    // Filter envelope to the cutoff.
    let bright = |amount: f32| {
        let mut h = filtered(1.0, 200.0, 0.1);
        h.set_param("env.filter.decay", 5_000.0);
        slot(&mut h, 0, "filter_env", "filter1_cutoff", None, amount);
        brightness(&hold(&mut h, 36, 0.2))
    };
    assert!(bright(1.0) > bright(0.0) * 2.0);
    assert!(bright(-1.0) <= bright(0.0));
    // Velocity to the level.
    let level = |amount: f32, vel: f32| {
        let mut h = synth();
        plain(&mut h);
        slot(&mut h, 0, "velocity", "level", None, amount);
        h.push_note("notes_in", on(0, 60, vel));
        rms(&render(&mut h, 0.3)[4_000..])
    };
    assert!(level(0.5, 1.0) > level(0.0, 1.0) * 1.3);
    assert!(level(-0.5, 1.0) < level(0.0, 1.0) * 0.7);
}

type Tweak<'a> = &'a dyn Fn(&mut DeviceHarness);

#[test]
fn a_slot_reaches_oscillator_level_and_shape_resonance_and_drive() {
    let play = |target_key: &str, amount: f32, set: &dyn Fn(&mut DeviceHarness)| {
        let mut h = synth();
        plain(&mut h);
        set(&mut h);
        slot(&mut h, 0, "velocity", target_key, None, amount);
        bits(&hold(&mut h, 48, 0.2))
    };
    let tweaks: [(&str, Tweak); 6] = [
        ("osc2_level", &|h| {
            h.set_param("osc2.level", 0.3);
            h.set_param("osc2.type", 3.0);
        }),
        ("osc1_shape", &|h| h.set_param("osc1.type", 1.0)),
        ("filter2_resonance", &|h| {
            h.set_param("filter2.mode", 1.0);
            h.set_param("filter2.cutoff", 500.0);
        }),
        ("filter3_cutoff", &|h| {
            h.set_param("filter3.mode", 1.0);
            h.set_param("filter3.cutoff", 500.0);
        }),
        ("shaper_drive", &|h| h.set_param("shaper.type", 1.0)),
        ("osc3_level", &|h| {
            h.set_param("osc3.level", 0.3);
            h.set_param("osc3.type", 3.0);
        }),
    ];
    for (key, set) in tweaks {
        assert_ne!(play(key, 0.8, set), play(key, 0.0, set), "{key}");
    }
}

#[test]
fn the_step_and_random_sources_modulate() {
    // The random source differs from note to note, the step source follows the pattern.
    let level = |seed_note: u8| {
        let mut h = synth();
        plain(&mut h);
        slot(&mut h, 0, "random", "level", None, 0.9);
        let mut v = Vec::new();
        for n in 0..4u16 {
            h.push_note("notes_in", on(n * 3, seed_note + n as u8, 1.0));
        }
        v.extend(render(&mut h, 0.1));
        rms(&v[2_000..])
    };
    assert!(level(60) > 0.0);
    let mut h = seq_synth();
    h.set_param("seq.pitch.1", 12.0);
    h.set_param("seq.pitch.2", -12.0);
    h.set_param("seq.length", 2.0);
    slot(&mut h, 0, "step", "pitch", None, 0.5);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let o = render(&mut h, 0.5);
    assert!(o.iter().all(|v| v.is_finite() && v.abs() < 4.0));
}

#[test]
fn the_random_source_draws_a_new_value_per_note_and_is_repeatable() {
    let play = || {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", 3.0);
        slot(&mut h, 0, "random", "level", None, 1.0);
        let mut out = Vec::new();
        for _ in 0..3 {
            h.push_note("notes_in", on(0, 60, 1.0));
            let o = render(&mut h, 0.15);
            out.push(rms(&o[3_000..6_000]));
            push(&mut h, 0, NoteKind::AllOff);
            render(&mut h, 0.05);
        }
        out
    };
    let a = play();
    assert_ne!(a[0].to_bits(), a[1].to_bits());
    assert_eq!(
        a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        play().iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn pressure_is_a_source() {
    let play = |pressure: f32| {
        let mut h = filtered(1.0, 200.0, 0.1);
        slot(&mut h, 0, "pressure", "filter1_cutoff", None, 1.0);
        h.push_note("notes_in", on(0, 48, 1.0));
        push(
            &mut h,
            1,
            NoteKind::PolyPressure {
                key: 48,
                value: pressure,
            },
        );
        brightness(&render(&mut h, 0.3)[6_000..])
    };
    assert!(play(1.0) > play(0.0) * 1.5);
}

#[test]
fn the_output_is_the_same_bit_for_bit_however_the_render_is_split() {
    let play = |chunks: &[usize]| {
        let mut h = synth();
        h.set_param("lfo1.rate", 3.0);
        h.set_param("filter2.mode", 5.0);
        h.set_param("shaper.type", 4.0);
        slot(&mut h, 0, "filter_env", "filter1_cutoff", None, 0.7);
        slot(&mut h, 1, "lfo1", "pitch", Some("mod_wheel"), 0.02);
        slot(&mut h, 2, "lfo2", "level", None, 0.3);
        slot(&mut h, 3, "step", "osc1_shape", Some("velocity"), 0.5);
        h.push_note("notes_in", on(0, 45, 0.9));
        h.push_note("notes_in", on(5, 52, 0.5));
        let mut out = Vec::new();
        for &n in chunks {
            out.extend(bits(&h.render(n, &OUT)[0]));
        }
        out
    };
    let whole = play(&[64]);
    assert_eq!(whole, play(&[1; 64]));
    assert_eq!(whole, play(&[7, 13, 44]));
}

#[test]
fn the_output_stays_finite_and_smooth_with_every_slot_at_full_depth() {
    let mut h = synth();
    h.set_param("filter1.resonance", 0.9);
    h.set_param("filter2.mode", 5.0);
    h.set_param("filter3.mode", 6.0);
    h.set_param("shaper.type", 1.0);
    for (n, (s, t)) in [
        ("lfo1", "filter1_cutoff"),
        ("lfo2", "pitch"),
        ("amp_env", "level"),
        ("filter_env", "filter2_cutoff"),
        ("mod_env", "pitch"),
        ("velocity", "shaper_drive"),
        ("key", "filter3_cutoff"),
        ("mod_wheel", "osc1_shape"),
    ]
    .into_iter()
    .enumerate()
    {
        slot(
            &mut h,
            n,
            s,
            t,
            Some("pressure"),
            if n % 2 == 0 { 1.0 } else { -1.0 },
        );
    }
    for k in [36u8, 48, 60, 84, 96] {
        h.push_note("notes_in", on(0, k, 1.0));
    }
    let o = render(&mut h, 0.5);
    assert!(o.iter().all(|v| v.is_finite() && v.abs() < 4.0));
    assert!(max_step(&o) < 2.0);
}

// -------------------------------------------------------------------------------- play

#[test]
fn voices_bound_the_polyphony_and_a_steal_does_not_click() {
    let play = |voices: f32| {
        let mut h = synth();
        plain(&mut h);
        h.set_param("voices", voices);
        h.set_param("osc1.type", 3.0);
        for (n, k) in [48u8, 52, 55, 59, 62, 66].into_iter().enumerate() {
            h.push_note("notes_in", on(n as u16 * 8, k, 1.0));
        }
        render(&mut h, 0.4)
    };
    let (two, six) = (play(2.0), play(6.0));
    assert!(rms(&six[8_000..]) > rms(&two[8_000..]) * 1.3);
    assert!(max_step(&two) < 0.5);
    // A seventh and eighth voice cost the same as a free voice: all eight are preallocated.
    assert!(rms(&play(8.0)[8_000..]) >= rms(&six[8_000..]) * 0.9);
}

#[test]
fn mono_plays_the_last_key_and_returns_to_the_one_held() {
    let play = |mode: f32| {
        let mut h = synth();
        plain(&mut h);
        h.set_param("osc1.type", 3.0);
        h.set_param("play.mode", mode);
        h.push_note("notes_in", on(0, 48, 1.0));
        h.push_note("notes_in", on(40, 60, 1.0));
        let first = render(&mut h, 0.4);
        push(&mut h, 0, NoteKind::Off { key: 60 });
        let second = render(&mut h, 0.4);
        (first, second)
    };
    for mode in [1.0, 2.0] {
        let (first, second) = play(mode);
        let top = dominant_frequency(&first[10_000..10_000 + 8_192], SR);
        assert!((top - hz(60)).abs() < 2.0, "mode {mode}: {top}");
        let back = dominant_frequency(&second[10_000..10_000 + 8_192], SR);
        assert!((back - hz(48)).abs() < 2.0, "mode {mode}: {back}");
    }
    // Poly sounds both keys.
    let (first, _) = play(0.0);
    let m = magnitude_spectrum(&first[10_000..10_000 + 8_192]);
    let bin = SR / 8_192.0;
    let at = |f: f64| m[(f / bin).round() as usize];
    assert!(at(hz(48)) > 100.0 && at(hz(60)) > 100.0);
}

#[test]
fn glide_slides_the_mono_pitch() {
    let mut h = synth();
    plain(&mut h);
    h.set_param("osc1.type", 3.0);
    h.set_param("play.mode", 2.0);
    h.set_param("play.glide", 400.0);
    h.push_note("notes_in", on(0, 48, 1.0));
    render(&mut h, 0.3);
    h.push_note("notes_in", on(0, 60, 1.0));
    let o = render(&mut h, 0.15);
    let early = dominant_frequency(&o[600..600 + 2_048], SR);
    let late = dominant_frequency(&o[o.len() - 2_048..], SR);
    assert!(early < hz(60) * 0.97 && early >= hz(48) * 0.99, "{early}");
    assert!(late > early, "{late} vs {early}");
}

// -------------------------------------------------------------------------------- sequencer

/// The batches (in samples) at which the gate output rises, over `secs`.
fn gate_edges(h: &mut DeviceHarness, secs: f64) -> (Vec<usize>, Vec<f32>) {
    let o = h.render(seconds(secs), &["seq_gate_cv_out", "seq_pitch_cv_out"]);
    let mut edges = Vec::new();
    let mut pitches = Vec::new();
    for n in 1..o[0].len() {
        if o[0][n] > 0.5 && o[0][n - 1] < 0.5 {
            edges.push(n);
            pitches.push(o[1][n]);
        }
    }
    if o[0][0] > 0.5 {
        edges.insert(0, 0);
        pitches.insert(0, o[1][0]);
    }
    (edges, pitches)
}

/// Samples per 1/16 at 120 BPM and 48 kHz.
const STEP: usize = 6_000;

#[test]
fn the_sequencer_plays_a_step_per_sixteenth_from_the_key() {
    let mut h = seq_synth();
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (edges, pitches) = gate_edges(&mut h, 1.0);
    assert_eq!(edges.len(), 8, "{edges:?}");
    for (n, e) in edges.iter().enumerate() {
        assert_eq!(*e, n * STEP, "step {n}");
    }
    // The default pattern, relative to middle C: 0 0 12 0 7 0 10 12.
    let want = [0.0, 0.0, 12.0, 0.0, 7.0, 0.0, 10.0, 12.0].map(|s: f32| s / 24.0);
    for (got, want) in pitches.iter().zip(want) {
        assert!((got - want).abs() < 1e-6, "{pitches:?}");
    }
}

#[test]
fn the_sequencer_sounds_and_the_keys_alone_do_not() {
    let mut h = seq_synth();
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 48,
            velocity: 1.0,
        },
    );
    let o = h.render(seconds(0.3), &OUT);
    assert!(rms(&o[0][2_000..]) > 0.01);
    // The pattern ends with the key.
    push(&mut h, 0, NoteKind::Off { key: 48 });
    h.render(seconds(0.2), &OUT);
    let tail = h.render(seconds(0.3), &OUT);
    assert!(peak(&tail[0]) < 1e-3);
    // With the sequencer off the key plays itself and the gate CV rests.
    let mut h = synth();
    plain(&mut h);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 48,
            velocity: 1.0,
        },
    );
    let o = h.render(seconds(0.3), &["out_l", "seq_gate_cv_out"]);
    assert!(rms(&o[0][2_000..]) > 0.01);
    assert_eq!(peak(&o[1]), 0.0);
}

#[test]
fn rate_length_swing_rests_and_transposition() {
    // 1/8: half as many steps.
    let mut h = seq_synth();
    h.set_param("seq.rate", 1.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (edges, _) = gate_edges(&mut h, 1.0);
    assert_eq!(edges.len(), 4);
    assert_eq!(edges[1], 2 * STEP);
    // Length 4 repeats the first four steps; a rest leaves a gap; the root transposes.
    let mut h = seq_synth();
    h.set_param("seq.length", 4.0);
    h.set_param("seq.on.2", 0.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 67,
            velocity: 1.0,
        },
    );
    let (edges, pitches) = gate_edges(&mut h, 1.0);
    assert_eq!(edges.len(), 6, "{edges:?}");
    assert_eq!(edges[1], 2 * STEP);
    assert_eq!(edges[3], 4 * STEP);
    assert!((pitches[0] - 7.0 / 24.0).abs() < 1e-6);
    assert!((pitches[1] - 19.0 / 24.0).abs() < 1e-6);
    assert!((pitches[3] - 7.0 / 24.0).abs() < 1e-6);
    // Swing 50 puts every odd step a quarter of a step late.
    let mut h = seq_synth();
    h.set_param("seq.swing", 50.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (edges, _) = gate_edges(&mut h, 0.5);
    assert_eq!(edges[1], STEP + STEP / 4);
    assert_eq!(edges[2], 2 * STEP);
}

#[test]
fn the_gate_length_and_a_tie() {
    let mut h = seq_synth();
    h.set_param("seq.gate", 50.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let o = h.render(seconds(0.25), &["seq_gate_cv_out"]);
    let high = o[0][..STEP].iter().filter(|v| **v > 0.5).count();
    assert_eq!(high, STEP / 2);
    // 100 % ties the steps: the gate never falls while the key is down.
    let mut h = seq_synth();
    h.set_param("seq.gate", 100.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let o = h.render(seconds(0.5), &["seq_gate_cv_out"]);
    assert!(o[0].iter().all(|v| *v > 0.5));
    // A tie in the mono modes glides without a click.
    let mut h = seq_synth();
    h.set_param("seq.gate", 100.0);
    h.set_param("play.mode", 2.0);
    h.set_param("osc1.type", 3.0);
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let o = render(&mut h, 0.5);
    assert!(rms(&o[2_000..]) > 0.05 && max_step(&o[2_000..]) < 1.0);
}

#[test]
fn the_start_is_sample_accurate_and_the_pattern_follows_the_tempo() {
    let at = |offset: u16, bpm: f64| {
        let mut h = seq_synth();
        h.time_mut().tempo = atomik_core::Tempo::new(bpm).unwrap();
        h.time_mut().ticks_per_frame = bpm * atomik_core::PPQ as f64 / (60.0 * SR);
        push(
            &mut h,
            offset,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        );
        gate_edges(&mut h, 0.5).0
    };
    let base = at(0, 120.0);
    let shifted = at(17, 120.0);
    assert_eq!(shifted[0], 17);
    assert_eq!(
        base.iter().map(|e| e + 17).collect::<Vec<_>>()[..shifted.len().min(base.len()) - 1],
        shifted[..shifted.len().min(base.len()) - 1]
    );
    // 240 BPM: twice the steps.
    assert_eq!(at(0, 240.0)[1], STEP / 2);
}

#[test]
fn the_pattern_restarts_with_each_run_and_stops_with_all_off() {
    let mut h = seq_synth();
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    gate_edges(&mut h, 0.3);
    push(&mut h, 0, NoteKind::Off { key: 60 });
    let (edges, _) = gate_edges(&mut h, 0.3);
    assert!(edges.is_empty());
    push(
        &mut h,
        10,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    let (edges, pitches) = gate_edges(&mut h, 0.2);
    assert_eq!(edges[0], 10);
    assert_eq!(pitches[0], 0.0);
    push(&mut h, 0, NoteKind::AllOff);
    let (edges, _) = gate_edges(&mut h, 0.3);
    assert!(edges.is_empty());
}

#[test]
fn switching_the_sequencer_off_ends_its_note() {
    let mut h = seq_synth();
    push(
        &mut h,
        0,
        NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    );
    h.render(seconds(0.1), &OUT);
    h.set_param("seq.run", 0.0);
    h.render(seconds(0.1), &OUT);
    let tail = h.render(seconds(0.2), &OUT);
    assert!(peak(&tail[0]) < 1e-3);
}

// -------------------------------------------------------------------------------- controllers

fn performance_synth() -> DeviceHarness {
    let mut h = synth();
    plain(&mut h);
    h.set_param("osc1.type", 3.0);
    h
}

#[test]
fn sustain_pedal_holds_the_release_until_pedal_up() {
    assert_sustain_pedal(performance_synth, &OUT, 57);
}

#[test]
fn volume_and_expression_scale_the_output() {
    assert_volume_and_expression(performance_synth, &OUT, 57);
}

#[test]
fn bend_range_and_rpn_zero() {
    assert_bend_range(performance_synth, &OUT, 57);
}

#[test]
fn the_mod_wheel_opens_the_filter() {
    let play = |wheel: f32| {
        let mut h = filtered(1.0, 400.0, 0.1);
        h.set_param("wheel.to_cutoff", 3.0);
        push(
            &mut h,
            0,
            NoteKind::Cc {
                controller: 1,
                value: wheel,
            },
        );
        brightness(&hold(&mut h, 48, 0.3)[6_000..])
    };
    assert!(play(1.0) > play(0.0) * 2.0);
}

#[test]
fn the_cvs_move_pitch_and_level_and_the_lfo_is_written_out() {
    let mut h = performance_synth();
    h.set_connected("pitch_cv_in", true);
    *h.input_mut("pitch_cv_in") = [0.5; BATCH];
    let o = hold(&mut h, 57, 0.6);
    // Half a unit of 24 semitones is an octave.
    let f = dominant_frequency(&o[8_000..8_000 + 16_384], SR);
    assert!((f / hz(57) - 2.0).abs() < 0.01, "{f}");
    let level = |cv: f32| {
        let mut h = performance_synth();
        h.set_connected("level_cv_in", true);
        *h.input_mut("level_cv_in") = [cv; BATCH];
        rms(&hold(&mut h, 57, 0.3)[4_000..])
    };
    assert!(level(-0.5) < level(0.0) * 0.4);
    let mut h = synth();
    h.set_param("lfo1.wave", 3.0);
    let o = h.render(8, &["lfo_cv_out"]);
    assert!(peak(&o[0]) > 0.5 && peak(&o[0]) <= 1.0);
}

// -------------------------------------------------------------------------------- guards

#[test]
fn nothing_allocates_while_params_change_and_notes_sound() {
    let mut h = synth();
    h.push_note("notes_in", on(0, 60, 0.7));
    h.render(2, &OUT);
    for (key, v) in [
        ("filter1.mode", 6.0),
        ("filter2.mode", 5.0),
        ("filter3.mode", 4.0),
        ("filter.routing", 2.0),
        ("shaper.type", 4.0),
        ("osc1.type", 6.0),
        ("osc2.type", 4.0),
        ("osc3.type", 5.0),
        ("play.mode", 2.0),
        ("voices", 2.0),
        ("seq.run", 1.0),
        ("seq.rate", 2.0),
        ("lfo1.wave", 4.0),
    ] {
        assert_no_alloc(|| h.set_param(key, v));
        h.push_note("notes_in", on(0, 50, 0.7));
        h.render(3, &OUT);
    }
    slot(&mut h, 0, "lfo1", "pitch", None, 0.5);
    h.render(3, &OUT);
    assert_no_alloc(|| h.device_mut().reset());
}

#[test]
fn extreme_settings_stay_finite_and_below_4() {
    let mut h = synth();
    for p in DESCRIPTOR.params {
        if p.key.starts_with("internal.") || p.key.starts_with("mod") {
            continue;
        }
        h.set_param(p.key, p.max);
    }
    h.set_param("filter1.mode", 5.0);
    h.set_param("filter2.mode", 6.0);
    h.set_param("filter3.mode", 1.0);
    h.set_param("seq.run", 0.0);
    for k in [12u8, 36, 60, 96, 127, 0, 72, 84] {
        h.push_note("notes_in", on(0, k, 1.0));
    }
    let o = render(&mut h, 0.6);
    assert!(o.iter().all(|v| v.is_finite() && v.abs() < 4.0));
}

#[test]
fn the_same_seed_renders_the_same_bits_and_another_seed_differs() {
    let play = |seed: u64| {
        let mut h = DeviceHarness::with_options(Box::new(SemiModular::new()), SR, 120.0, seed);
        h.set_param("osc1.type", 7.0);
        h.set_param("lfo1.wave", 4.0);
        slot(&mut h, 0, "random", "level", None, 0.5);
        for n in 0..4u16 {
            h.push_note("notes_in", on(n * 5, 48 + n as u8 * 4, 0.9));
        }
        bits(&render(&mut h, 0.3))
    };
    assert_eq!(play(7), play(7));
    assert_ne!(play(7), play(8));
}

#[test]
fn reset_silences_tails() {
    let mut h = synth();
    h.push_note("notes_in", on(0, 48, 1.0));
    h.render(10, &OUT);
    h.device_mut().reset();
    let o = h.render(4, &OUT);
    assert!(o.iter().all(|c| peak(c) == 0.0));
    let _ = off(0, 0);
}
