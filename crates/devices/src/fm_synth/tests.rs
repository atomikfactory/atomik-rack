//! Tests of the FM synth: the routings against a reference computation, frequencies, feedback,
//! voices, envelopes, aliasing, the mono modes, the CVs and the factory sounds.

use super::*;
use crate::test_support::{
    SR, assert_bend_range, assert_keys, assert_sustain_pedal, assert_volume_and_expression,
    harness, off, on, sweep_params_min_max,
};
use atomik_core::NoteEvent;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::{db, dominant_frequency, magnitude_spectrum, max_step, peak, rms};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Test-only window into the voice allocator, updated at the end of every `process` (atomic
/// stores only: no allocation).
#[derive(Debug, Default)]
pub struct VoiceProbe {
    /// Number of non-idle voices.
    pub active: AtomicUsize,
    /// Per voice: 0 when idle, else 1 + the key it is playing (or will play after a steal).
    pub keys: [AtomicU32; MAX_VOICES],
}

impl VoiceProbe {
    fn keys(&self) -> Vec<u8> {
        let mut k: Vec<u8> = self
            .keys
            .iter()
            .map(|a| a.load(Ordering::Relaxed))
            .filter(|v| *v > 0)
            .map(|v| (v - 1) as u8)
            .collect();
        k.sort_unstable();
        k
    }
    fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
}

const OUT: [&str; 2] = ["out_l", "out_r"];

fn synth() -> (DeviceHarness, Arc<VoiceProbe>) {
    let probe = Arc::new(VoiceProbe::default());
    let mut s = FmSynth::new();
    s.probe = Some(probe.clone());
    (harness(Box::new(s)), probe)
}

fn seconds(s: f64) -> usize {
    (s * SR / BATCH as f64).ceil() as usize
}

fn push(h: &mut DeviceHarness, offset: u16, kind: NoteKind) {
    h.push_note("notes_in", NoteEvent { offset, kind });
}

/// A neutral patch: the algorithm, the four operators at `levels` and `coarse` ratios, flat
/// envelopes (sustain 1, a 0.5 ms attack), no velocity, key scaling, feedback, LFO, pitch
/// envelope or wheel, master level 0 dB.
fn patch(h: &mut DeviceHarness, algorithm: usize, levels: [f32; 4], coarse: [f32; 4]) {
    h.set_param("algorithm", algorithm as f32);
    h.set_param("feedback", 0.0);
    h.set_param("index", 1.0);
    h.set_param("wheel.to_index", 0.0);
    h.set_param("master.level", 0.0);
    for n in 0..4 {
        let k = |f: &str| format!("op{}.{f}", n + 1);
        h.set_param(&k("fixed"), 0.0);
        h.set_param(&k("coarse"), coarse[n]);
        h.set_param(&k("fine"), 0.0);
        h.set_param(&k("detune"), 0.0);
        h.set_param(&k("level"), levels[n]);
        h.set_param(&k("velocity"), 0.0);
        h.set_param(&k("key_scale"), 0.0);
        h.set_param(&k("delay"), 0.0);
        h.set_param(&k("attack"), 0.5);
        h.set_param(&k("decay"), 100.0);
        h.set_param(&k("sustain"), 1.0);
        h.set_param(&k("release"), 100.0);
    }
}

/// One operator alone: a sine at `coarse` times the key, at full level.
fn pure(h: &mut DeviceHarness) {
    patch(h, 0, [1.0, 0.0, 0.0, 0.0], [1.0; 4]);
}

fn hz(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// RMS of the first difference: weights high harmonics.
fn brightness(x: &[f32]) -> f32 {
    let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
    rms(&d)
}

// -------------------------------------------------------------------------------- descriptor

#[test]
fn descriptor_validates() {
    let mut params: Vec<String> = ["sound", "algorithm", "feedback", "index"]
        .map(String::from)
        .to_vec();
    for n in 1..=4 {
        for f in [
            "fixed",
            "coarse",
            "fine",
            "detune",
            "level",
            "velocity",
            "key_scale",
            "delay",
            "attack",
            "decay",
            "sustain",
            "release",
        ] {
            params.push(format!("op{n}.{f}"));
        }
    }
    params.extend(
        [
            "pitch.env_amount",
            "pitch.env_decay",
            "lfo.wave",
            "lfo.rate",
            "lfo.to_pitch",
            "lfo.to_amp",
            "lfo.to_index",
            "wheel.to_index",
            "play.mode",
            "play.glide",
            "voices",
            "spread",
            "master.level",
            "pitch_cv_trim",
            "cv.index_amount",
            "master.level_cv_trim",
            "pan_cv_trim",
            "bend.range",
        ]
        .map(String::from),
    );
    let keys: Vec<&str> = params.iter().map(String::as_str).collect();
    assert_keys(
        &DESCRIPTOR,
        &keys,
        &[
            "notes_in",
            "pitch_cv_in",
            "index_cv_in",
            "level_cv_in",
            "pan_cv_in",
            "out_l",
            "out_r",
            "lfo_cv_out",
        ],
    );
    assert_eq!(PARAMS.len(), 70);
    assert_eq!(DESCRIPTOR.type_id, "atomik.fm-synth");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
    assert_eq!(
        DESCRIPTOR.param_by_key("algorithm").unwrap().unit,
        Unit::Choice(&ALGORITHM_NAMES)
    );
    assert_eq!(
        DESCRIPTOR.param_by_key("sound").unwrap().unit,
        Unit::Choice(&sounds::SOUND_NAMES)
    );
    assert_eq!(DESCRIPTOR.param_by_key("voices").unwrap().default, 8.0);
    assert_eq!(DESCRIPTOR.param_by_key("voices").unwrap().max, 16.0);
    // The ids are the indices, and op_param_id finds them.
    for (i, p) in DESCRIPTOR.params.iter().enumerate() {
        assert_eq!(p.id.0 as usize, i, "{}", p.key);
    }
    assert_eq!(
        DESCRIPTOR.params[op_param_id(2, F_RELEASE) as usize].key,
        "op3.release"
    );
}

// -------------------------------------------------------------------------------- routing

/// The routings written out once more, as edges `(modulator, target)` (1-based) and carriers,
/// independent of the table the device reads.
fn routing(alg: usize) -> (Vec<(usize, usize)>, Vec<usize>) {
    match alg {
        0 => (vec![(4, 3), (3, 2), (2, 1)], vec![1]),
        1 => (vec![(4, 3), (2, 1)], vec![3, 1]),
        2 => (vec![(4, 2), (3, 2), (2, 1)], vec![1]),
        3 => (vec![(4, 1), (4, 2), (4, 3)], vec![1, 2, 3]),
        4 => (vec![(4, 1), (3, 1), (2, 1)], vec![1]),
        5 => (vec![(4, 3), (3, 2)], vec![2, 1]),
        6 => (vec![(4, 3), (3, 2), (3, 1)], vec![2, 1]),
        7 => (vec![(4, 3), (3, 1), (2, 1)], vec![1]),
        8 => (vec![(4, 1), (4, 2), (3, 1), (3, 2)], vec![1, 2]),
        9 => (vec![(4, 3)], vec![3, 2, 1]),
        10 => (vec![(4, 3), (3, 2), (2, 1)], vec![2, 1]),
        _ => (vec![], vec![1, 2, 3, 4]),
    }
}

/// The voice rendered naively in `f64`, sample by sample: operator 4 first, the phase of every
/// operator moved by the outputs of its modulators, the carriers summed with `1/sqrt(n)`, times
/// the voice gain. `ratios` are frequency ratios of the key, `levels` linear.
#[allow(clippy::too_many_arguments)]
fn reference(
    alg: usize,
    ratios: [f64; 4],
    levels: [f64; 4],
    index: f64,
    feedback: f64,
    key: u8,
    n: usize,
) -> Vec<f32> {
    let (edges, carriers) = routing(alg);
    let mut phase = [0.0f64; 4];
    let mut fb = [0.0f64; 2];
    let to_cycles = 1.0 / std::f64::consts::TAU;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut y = [0.0f64; 4];
        for op in (0..4).rev() {
            let mut offset = 0.0;
            for &(from, to) in &edges {
                if to == op + 1 {
                    offset += y[from - 1] * index * f64::from(MAX_INDEX_RAD) * to_cycles;
                }
            }
            if op == 3 {
                offset +=
                    0.5 * (fb[0] + fb[1]) * feedback * f64::from(MAX_FEEDBACK_RAD) * to_cycles;
            }
            y[op] = (std::f64::consts::TAU * (phase[op] + offset)).sin() * levels[op];
        }
        fb = [y[3], fb[0]];
        let sum: f64 = carriers.iter().map(|c| y[c - 1]).sum();
        out.push((sum / (carriers.len() as f64).sqrt() * f64::from(VOICE_GAIN)) as f32);
        for op in 0..4 {
            phase[op] = (phase[op] + hz(key) * ratios[op] / SR).fract();
        }
    }
    out
}

fn rms_diff(a: &[f32], b: &[f32]) -> f32 {
    let d: Vec<f32> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    rms(&d)
}

const LEVELS: [f32; 4] = [0.35, 0.3, 0.25, 0.2];
const COARSE: [f32; 4] = [1.0, 2.0, 3.0, 4.0];

fn rendered(algorithm: usize, feedback: f32, key: u8, secs: f64) -> Vec<f32> {
    let (mut h, _) = synth();
    patch(&mut h, algorithm, LEVELS, COARSE);
    h.set_param("feedback", feedback);
    h.push_note("notes_in", on(0, key, 1.0));
    h.render(seconds(secs), &OUT).remove(0)
}

fn reference_of(algorithm: usize, feedback: f64, key: u8, n: usize) -> Vec<f32> {
    reference(
        algorithm,
        [1.0, 2.0, 3.0, 4.0],
        LEVELS.map(f64::from),
        1.0,
        feedback,
        key,
        n,
    )
}

#[test]
fn every_algorithm_matches_the_reference_computation() {
    // Key 36 (65 Hz) with ratios up to 4: far from the bandwidth limit, so the limiter is idle.
    let mut references = Vec::new();
    for (alg, name) in ALGORITHM_NAMES.iter().enumerate() {
        let o = rendered(alg, 0.0, 36, 0.4);
        let r = reference_of(alg, 0.0, 36, o.len());
        let err = rms_diff(&o[2_400..], &r[2_400..]);
        assert!(
            err < 1e-3,
            "{}: rms error {err} against peak {}",
            name,
            peak(&o)
        );
        assert!(peak(&o) > 0.05, "{}: audible", name);
        references.push(r);
    }
    // The check has teeth: no algorithm's output is another algorithm's reference.
    for (alg, name) in ALGORITHM_NAMES.iter().enumerate() {
        let o = rendered(alg, 0.0, 36, 0.4);
        for (other, r) in references.iter().enumerate() {
            if other != alg {
                let d = rms_diff(&o[2_400..], &r[2_400..]);
                assert!(
                    d > 1e-3,
                    "{} is indistinguishable from {}: {d}",
                    name,
                    ALGORITHM_NAMES[other]
                );
            }
        }
    }
}

#[test]
fn feedback_matches_the_reference_computation() {
    // A mild feedback (beta 0.27 rad) is a contraction: the transient of the attack dies out.
    for alg in [0, 11] {
        let o = rendered(alg, 0.15, 36, 0.4);
        let r = reference_of(alg, 0.15, 36, o.len());
        let err = rms_diff(&o[4_800..], &r[4_800..]);
        assert!(err < 1e-3, "{}: {err}", ALGORITHM_NAMES[alg]);
    }
}

#[test]
fn the_index_scales_every_modulator_and_zero_leaves_the_carriers() {
    let at = |index: f32| {
        let (mut h, _) = synth();
        patch(&mut h, 0, [0.35, 0.3, 0.0, 0.0], COARSE);
        h.set_param("index", index);
        h.push_note("notes_in", on(0, 48, 1.0));
        h.render(seconds(0.4), &OUT).remove(0)
    };
    // Index 0: operator 1 alone, a pure sine at its level.
    let flat = at(0.0);
    let sine: Vec<f32> = reference(0, [1.0; 4], [0.35, 0.0, 0.0, 0.0], 0.0, 0.0, 48, flat.len());
    assert!(rms_diff(&flat[2_400..], &sine[2_400..]) < 1e-3);
    // More index, more partials: the brightness rises.
    assert!(brightness(&at(1.0)[2_400..]) > brightness(&flat[2_400..]) * 1.5);
    assert!(brightness(&at(2.0)[2_400..]) > brightness(&at(1.0)[2_400..]));
}

// ------------------------------------------------------------------------------- frequencies

fn freq_of(setup: impl Fn(&mut DeviceHarness), key: u8) -> f64 {
    let (mut h, _) = synth();
    pure(&mut h);
    setup(&mut h);
    h.push_note("notes_in", on(0, key, 1.0));
    let o = h.render(seconds(1.0), &OUT);
    dominant_frequency(&o[0][4_800..4_800 + 32_768], SR)
}

#[test]
fn key_69_is_440_hz_and_the_ratio_steps_are_exact() {
    assert!((freq_of(|_| {}, 69) - 440.0).abs() < 0.5);
    assert!((freq_of(|_| {}, 57) - 220.0).abs() < 0.5);
    // Coarse 0 is the ratio 0.5, coarse n is n.
    assert!((freq_of(|h| h.set_param("op1.coarse", 0.0), 69) - 220.0).abs() < 0.5);
    assert!((freq_of(|h| h.set_param("op1.coarse", 3.0), 57) - 660.0).abs() < 0.5);
    assert!((freq_of(|h| h.set_param("op1.coarse", 12.0), 45) - 1_320.0).abs() < 1.0);
    // The fine fraction adds to the ratio.
    let f = |h: &mut DeviceHarness| {
        h.set_param("op1.coarse", 3.0);
        h.set_param("op1.fine", 0.5);
    };
    assert!((freq_of(f, 57) - 770.0).abs() < 1.0);
}

#[test]
fn detune_is_in_cents() {
    let base = freq_of(|_| {}, 69);
    let up = freq_of(|h| h.set_param("op1.detune", 50.0), 69);
    let down = freq_of(|h| h.set_param("op1.detune", -50.0), 69);
    assert!((up / base - 2f64.powf(50.0 / 1_200.0)).abs() < 1e-3, "{up}");
    assert!(
        (down / base - 2f64.powf(-50.0 / 1_200.0)).abs() < 1e-3,
        "{down}"
    );
}

#[test]
fn a_fixed_operator_ignores_the_key() {
    let fixed = |h: &mut DeviceHarness| {
        h.set_param("op1.fixed", 1.0);
        h.set_param("op1.coarse", 24.0);
    };
    // 10^(24/8) = 1000 Hz at any key.
    for key in [36u8, 69, 96] {
        let f = freq_of(fixed, key);
        assert!((f - 1_000.0).abs() < 1.5, "key {key}: {f}");
    }
}

/// One steady operator with a quick release.
fn performance_synth() -> DeviceHarness {
    let (mut h, _) = synth();
    pure(&mut h);
    for n in 1..=4 {
        h.set_param(&format!("op{n}.release"), 5.0);
    }
    h
}

#[test]
fn sustain_pedal_holds_the_release_until_pedal_up() {
    assert_sustain_pedal(performance_synth, &OUT, 57);
}

#[test]
fn sustain_pedal_holds_the_mono_voice_and_the_key_stack() {
    let mut s = FmSynth::new();
    s.set_param(ParamId(P_MODE), 1.0);
    let pedal = |value| NoteKind::Cc {
        controller: 64,
        value,
    };
    s.handle(pedal(1.0));
    s.handle(NoteKind::On {
        key: 60,
        velocity: 1.0,
    });
    s.handle(NoteKind::Off { key: 60 });
    assert!(s.voices[0].gated, "the pedal holds the mono voice");
    s.handle(pedal(0.0));
    assert!(!s.voices[0].gated, "pedal up releases it");
}

#[test]
fn volume_and_expression_scale_the_output() {
    assert_volume_and_expression(performance_synth, &OUT, 57);
}

#[test]
fn bend_range_param_and_rpn_zero() {
    assert_bend_range(performance_synth, &OUT, 57);
}

#[test]
fn pressure_reaches_the_modulation_source() {
    let mut s = FmSynth::new();
    s.handle(NoteKind::ChannelPressure { value: 0.5 });
    s.handle(NoteKind::PolyPressure {
        key: 60,
        value: 0.75,
    });
    assert_eq!((s.ctl.pressure(60), s.ctl.pressure(61)), (0.75, 0.5));
    s.reset();
    assert_eq!(s.ctl.pressure(60), 0.0);
}

#[test]
fn pitch_bend_and_pitch_cv_move_the_pitch() {
    let (mut h, _) = synth();
    pure(&mut h);
    h.push_note("notes_in", on(0, 69, 1.0));
    push(&mut h, 0, NoteKind::PitchBend { value: 1.0 });
    let o = h.render(seconds(1.0), &OUT);
    let f = dominant_frequency(&o[0][4_800..4_800 + 32_768], SR);
    assert!((f - 440.0 * 2f64.powf(2.0 / 12.0)).abs() < 1.0, "bend {f}");

    // +0.5 of the 24-semitone CV range is one octave.
    let (mut h, _) = synth();
    pure(&mut h);
    *h.input_mut("pitch_cv_in") = [0.5; BATCH];
    h.push_note("notes_in", on(0, 57, 1.0));
    let o = h.render(seconds(1.0), &OUT);
    let f = dominant_frequency(&o[0][4_800..4_800 + 32_768], SR);
    assert!((f - 440.0).abs() < 1.0, "pitch CV {f}");
}

#[test]
fn the_pitch_envelope_falls_to_the_note() {
    let (mut h, _) = synth();
    pure(&mut h);
    h.set_param("pitch.env_amount", 12.0);
    h.set_param("pitch.env_decay", 100.0);
    h.push_note("notes_in", on(0, 57, 1.0));
    let o = h.render(seconds(1.0), &OUT).remove(0);
    let early = dominant_frequency(&o[0..2_048], SR);
    let late = dominant_frequency(&o[30_000..30_000 + 8_192], SR);
    assert!(early > 300.0 && early < 450.0, "early {early}");
    assert!((late - 220.0).abs() < 1.5, "late {late}");
}

#[test]
fn the_lfo_vibrato_swings_the_pitch_and_the_tremolo_the_level() {
    let (mut h, _) = synth();
    pure(&mut h);
    h.set_param("lfo.rate", 5.0);
    h.set_param("lfo.to_pitch", 12.0);
    h.push_note("notes_in", on(0, 57, 1.0));
    let o = h.render(seconds(1.0), &OUT).remove(0);
    let freqs: Vec<f64> = (0..8)
        .map(|i| dominant_frequency(&o[4_800 + i * 4_000..4_800 + i * 4_000 + 2_048], SR))
        .collect();
    let (lo, hi) = freqs
        .iter()
        .fold((f64::MAX, 0.0f64), |(l, h), f| (l.min(*f), h.max(*f)));
    assert!(hi / lo > 1.3, "{freqs:?}");

    let (mut h, _) = synth();
    pure(&mut h);
    h.set_param("lfo.rate", 5.0);
    h.set_param("lfo.to_amp", 1.0);
    h.push_note("notes_in", on(0, 57, 1.0));
    let o = h.render(seconds(1.0), &OUT).remove(0);
    let w = 2_000;
    let levels: Vec<f32> = (4..20).map(|i| peak(&o[i * w..(i + 1) * w])).collect();
    let (lo, hi) = levels
        .iter()
        .fold((f32::MAX, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
    assert!(lo < hi * 0.2, "{levels:?}");
}

#[test]
fn the_lfo_is_written_to_the_cv_output_every_sample() {
    let (mut h, _) = synth();
    h.set_param("lfo.rate", 10.0);
    h.render(3, &OUT);
    let a = h.output("lfo_cv_out").to_vec();
    assert!(a.iter().any(|v| v.abs() > 1e-3), "moves");
    assert!(a.iter().all(|v| v.abs() <= 1.0));
}

// ---------------------------------------------------------------------------------- feedback

/// Operator 4 alone at full level (a carrier in the additive routing), with `feedback`.
fn feedback_tone(key: u8, feedback: f32) -> Vec<f32> {
    let (mut h, _) = synth();
    patch(&mut h, 11, [0.0, 0.0, 0.0, 1.0], [1.0; 4]);
    h.set_param("feedback", feedback);
    h.push_note("notes_in", on(0, key, 1.0));
    h.render(seconds(1.0), &OUT).remove(0)
}

/// The energy of the bins that are not near a harmonic of `f0`, relative to the harmonics'.
fn off_harmonic_ratio(x: &[f32], f0: f64) -> f64 {
    let n = 1 << 15;
    let mags = magnitude_spectrum(&x[9_600..9_600 + n]);
    let bin = SR / n as f64;
    let (mut on_harmonic, mut off_harmonic) = (0.0f64, 0.0f64);
    for (k, m) in mags.iter().enumerate().skip(8) {
        let h = (k as f64 * bin / f0).round();
        if h >= 1.0 && (k as f64 * bin - h * f0).abs() < 6.0 * bin {
            on_harmonic += m * m;
        } else {
            off_harmonic += m * m;
        }
    }
    off_harmonic / on_harmonic
}

#[test]
fn feedback_makes_a_sine_a_saw_like_tone_and_stays_bounded() {
    let none = feedback_tone(45, 0.0);
    let full = feedback_tone(45, 1.0);
    assert!(
        peak(&full) <= 0.25 + 1e-4,
        "bounded by the level: {}",
        peak(&full)
    );
    assert!(full.iter().all(|s| s.is_finite()));
    assert!(brightness(&full[4_800..]) > brightness(&none[4_800..]) * 2.0);
    // The second harmonic is real: the sine has become a saw-like tone.
    let n = 1 << 15;
    let mags = magnitude_spectrum(&full[9_600..9_600 + n]);
    let bin = SR / n as f64;
    let at = |h: f64| mags[(h * hz(45) / bin).round() as usize];
    assert!(at(2.0) > at(1.0) * 0.3, "{} vs {}", at(2.0), at(1.0));
    // The brightness grows with the amount.
    let mut last = 0.0;
    for fb in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let b = brightness(&feedback_tone(33, fb)[4_800..]);
        assert!(b > last, "{fb}: {b} after {last}");
        last = b;
    }
}

#[test]
fn feedback_stays_tonal_at_its_maximum_on_every_register() {
    // The averaged feedback path: at the top of the range the output has partials at multiples of
    // the pitch and no noise from a loop gone unstable.
    for key in [33u8, 45, 57, 69, 81] {
        let x = feedback_tone(key, 1.0);
        let ratio = off_harmonic_ratio(&x, hz(key));
        assert!(ratio < 1e-3, "key {key}: off-harmonic energy {ratio:e}");
    }
}

// ------------------------------------------------------------------------------------ voices

#[test]
fn seventeen_notes_steal_the_oldest_voice() {
    let (mut h, probe) = synth();
    h.set_param("voices", 16.0);
    for (i, key) in (40u8..=56).enumerate() {
        h.push_note("notes_in", on(i as u16, key, 0.8));
    }
    let o = h.render(4, &OUT); // more than the 2 ms steal fade
    assert!(o.iter().flatten().all(|s| s.is_finite()));
    assert!(probe.active() <= 16, "active {}", probe.active());
    assert_eq!(
        probe.keys(),
        (41u8..=56).collect::<Vec<_>>(),
        "key 40 was stolen"
    );
    // Later: the oldest is now 41.
    h.push_note("notes_in", on(0, 70, 0.8));
    h.render(4, &OUT);
    assert!(!probe.keys().contains(&41) && probe.keys().contains(&70));
    assert_eq!(probe.active(), 16);
}

#[test]
fn voices_param_limits_polyphony() {
    let (mut h, probe) = synth();
    h.set_param("voices", 2.0);
    for (i, key) in [60u8, 64, 67].iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *key, 0.8));
    }
    h.render(4, &OUT);
    assert_eq!(probe.active(), 2);
    assert_eq!(probe.keys(), vec![64, 67]);
}

#[test]
fn lowering_voices_fades_out_the_extra_voices() {
    let (mut h, probe) = synth();
    for (i, key) in [60u8, 64, 67, 71].iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *key, 0.8));
    }
    h.render(4, &OUT);
    assert_eq!(probe.active(), 4);
    h.set_param("voices", 2.0);
    h.render(4, &OUT);
    assert_eq!(probe.active(), 2);
}

#[test]
fn same_key_retriggers_the_same_voice_continuously() {
    let (mut h, probe) = synth();
    pure(&mut h);
    h.set_param("op1.coarse", 0.0);
    h.set_param("op1.attack", 5.0);
    h.push_note("notes_in", on(0, 45, 1.0)); // 55 Hz, half that: small sample steps
    let mut out = h.render(seconds(0.3), &OUT).remove(0);
    h.push_note("notes_in", off(0, 45));
    out.extend(h.render(seconds(0.05), &OUT).remove(0));
    h.push_note("notes_in", on(10, 45, 0.3));
    out.extend(h.render(seconds(0.3), &OUT).remove(0));
    assert_eq!(probe.active(), 1);
    assert_eq!(probe.keys(), vec![45]);
    assert!(max_step(&out) < 0.02, "max step {}", max_step(&out));
}

#[test]
fn a_steal_fades_over_2ms_without_a_click() {
    let (mut h, probe) = synth();
    pure(&mut h);
    h.set_param("op1.coarse", 0.0);
    h.set_param("voices", 1.0);
    h.push_note("notes_in", on(0, 45, 1.0));
    let mut out = h.render(seconds(0.2), &OUT).remove(0);
    h.push_note("notes_in", on(5, 48, 1.0));
    out.extend(h.render(seconds(0.2), &OUT).remove(0));
    assert_eq!(probe.keys(), vec![48]);
    assert!(max_step(&out) < 0.02, "max step {}", max_step(&out));
}

#[test]
fn all_off_releases_every_voice_and_a_note_off_only_its_own() {
    let (mut h, probe) = synth();
    for (i, key) in [60u8, 64, 67].iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *key, 0.8));
    }
    h.render(seconds(0.1), &OUT);
    h.push_note("notes_in", off(0, 64));
    h.render(seconds(1.0), &OUT);
    assert_eq!(probe.keys(), vec![60, 67]);
    push(&mut h, 0, NoteKind::AllOff);
    h.render(seconds(1.0), &OUT);
    assert_eq!(probe.active(), 0);
}

#[test]
fn a_note_on_is_sample_accurate() {
    let (mut h, _) = synth();
    pure(&mut h);
    h.push_note("notes_in", on(37, 69, 1.0));
    let o = h.render(1, &OUT).remove(0);
    assert!(o[..37].iter().all(|s| *s == 0.0));
    assert!(o[37..].iter().any(|s| *s != 0.0));
}

// ----------------------------------------------------------------------------------- envelopes

#[test]
fn the_attack_and_release_follow_the_operator_envelope() {
    let (mut h, probe) = synth();
    pure(&mut h);
    h.set_param("op1.attack", 200.0);
    h.set_param("op1.release", 300.0);
    h.push_note("notes_in", on(0, 69, 1.0));
    let o = h.render(seconds(1.0), &OUT).remove(0);
    let early = peak(&o[0..480]); // the first 10 ms
    let late = peak(&o[24_000..30_000]);
    assert!(early < 0.35 * late, "{early} vs {late}");
    h.push_note("notes_in", off(0, 69));
    let tail = h.render(seconds(1.0), &OUT).remove(0);
    assert!(
        peak(&tail[0..480]) > 0.1,
        "still sounds just after the note off"
    );
    assert!(db(peak(&tail[tail.len() - BATCH..])) < -90.0);
    assert_eq!(probe.active(), 0);
}

#[test]
fn the_decay_falls_to_the_sustain_level() {
    let (mut h, _) = synth();
    pure(&mut h);
    h.set_param("op1.decay", 100.0);
    h.set_param("op1.sustain", 0.25);
    h.push_note("notes_in", on(0, 69, 1.0));
    let o = h.render(seconds(1.0), &OUT).remove(0);
    // The peak is the voice gain (0.5) times the envelope.
    let start = peak(&o[0..400]);
    let end = peak(&o[30_000..40_000]);
    assert!((start - 0.5).abs() < 0.05, "{start}");
    assert!((end - 0.125).abs() < 0.005, "{end}");
}

#[test]
fn an_operator_delay_holds_it_back() {
    let (mut h, probe) = synth();
    pure(&mut h);
    h.set_param("op1.delay", 200.0);
    h.push_note("notes_in", on(0, 69, 1.0));
    let o = h.render(seconds(0.5), &OUT).remove(0);
    assert!(o[..9_500].iter().all(|s| *s == 0.0), "silent for the delay");
    assert_eq!(probe.active(), 1, "the voice waits for its delay");
    assert!(peak(&o[9_700..]) > 0.2, "then it sounds");
    // A note off during the delay cancels the note.
    let (mut h, probe) = synth();
    pure(&mut h);
    h.set_param("op1.delay", 200.0);
    h.push_note("notes_in", on(0, 69, 1.0));
    h.render(seconds(0.05), &OUT);
    h.push_note("notes_in", off(0, 69));
    let o = h.render(seconds(0.5), &OUT).remove(0);
    assert!(peak(&o) == 0.0 && probe.active() == 0);
}

#[test]
fn a_modulator_envelope_shapes_the_brightness_over_the_note() {
    // A bell-like decay of the modulator: bright at the start, purer later.
    let (mut h, _) = synth();
    patch(&mut h, 0, [0.5, 0.6, 0.0, 0.0], [1.0, 3.0, 1.0, 1.0]);
    h.set_param("op2.decay", 200.0);
    h.set_param("op2.sustain", 0.0);
    h.push_note("notes_in", on(0, 48, 1.0));
    let o = h.render(seconds(1.0), &OUT).remove(0);
    let ratio = |a: &[f32]| brightness(a) / rms(a);
    assert!(ratio(&o[240..4_800]) > ratio(&o[40_000..45_000]) * 3.0);
}

// ------------------------------------------------------------------ velocity and key scaling

#[test]
fn velocity_scales_the_level_of_an_operator() {
    let at = |sens: f32, velocity: f32| {
        let (mut h, _) = synth();
        pure(&mut h);
        h.set_param("op1.velocity", sens);
        h.push_note("notes_in", on(0, 69, velocity));
        peak(&h.render(seconds(0.3), &OUT)[0])
    };
    assert!((at(0.0, 0.2) - at(0.0, 1.0)).abs() < 1e-3, "no sensitivity");
    assert!((at(1.0, 0.5) / at(1.0, 1.0) - 0.5).abs() < 0.02);
    assert!(at(0.5, 0.2) < at(0.5, 1.0));
}

#[test]
fn velocity_on_a_modulator_makes_a_harder_key_brighter() {
    let at = |velocity: f32| {
        let (mut h, _) = synth();
        patch(&mut h, 0, [0.5, 0.6, 0.0, 0.0], [1.0, 2.0, 1.0, 1.0]);
        h.set_param("op2.velocity", 1.0);
        h.push_note("notes_in", on(0, 48, velocity));
        let o = h.render(seconds(0.3), &OUT).remove(0);
        brightness(&o[4_800..]) / rms(&o[4_800..])
    };
    assert!(at(1.0) > at(0.3) * 1.3);
}

#[test]
fn key_scaling_lowers_the_level_on_one_side_of_middle_c() {
    let level = |scale: f32, key: u8| {
        let (mut h, _) = synth();
        pure(&mut h);
        h.set_param("op1.key_scale", scale);
        h.push_note("notes_in", on(0, key, 1.0));
        peak(&h.render(seconds(0.3), &OUT)[0])
    };
    let steps = 24.0; // two octaves above middle C
    let want = 10f32.powf(-0.4 * steps / 20.0);
    assert!((level(1.0, 84) / level(0.0, 84) - want).abs() < 0.01);
    assert!(
        (level(1.0, 48) - level(0.0, 48)).abs() < 1e-3,
        "no effect below"
    );
    assert!((level(-1.0, 36) / level(0.0, 36) - want).abs() < 0.01);
    assert!(
        (level(-1.0, 72) - level(0.0, 72)).abs() < 1e-3,
        "no effect above"
    );
    assert!((level(0.5, 84) / level(0.0, 84) - want.sqrt()).abs() < 0.01);
}

// ------------------------------------------------------------------------ modulation and CVs

#[test]
fn the_mod_wheel_and_the_index_cv_open_the_index() {
    let bright = |wheel: f32, cv: f32| {
        let (mut h, _) = synth();
        patch(&mut h, 0, [0.5, 0.4, 0.0, 0.0], [1.0, 2.0, 1.0, 1.0]);
        h.set_param("index", 0.2);
        h.set_param("wheel.to_index", 1.0);
        *h.input_mut("index_cv_in") = [cv; BATCH];
        push(
            &mut h,
            0,
            NoteKind::Cc {
                controller: 1,
                value: wheel,
            },
        );
        h.push_note("notes_in", on(0, 48, 1.0));
        let o = h.render(seconds(0.3), &OUT).remove(0);
        brightness(&o[4_800..])
    };
    assert!(bright(1.0, 0.0) > bright(0.0, 0.0) * 1.5, "wheel");
    assert!(bright(0.0, 1.0) > bright(0.0, 0.0) * 1.5, "CV");
    assert!(
        bright(0.0, -1.0) < bright(0.0, 0.0),
        "negative CV closes it"
    );
    // The wheel survives AllOff; a reset puts it back.
    let (mut h, _) = synth();
    patch(&mut h, 0, [0.5, 0.4, 0.0, 0.0], [1.0, 2.0, 1.0, 1.0]);
    h.set_param("index", 0.2);
    h.set_param("wheel.to_index", 1.0);
    push(
        &mut h,
        0,
        NoteKind::Cc {
            controller: 1,
            value: 1.0,
        },
    );
    push(&mut h, 1, NoteKind::AllOff);
    h.render(2, &OUT);
    h.push_note("notes_in", on(0, 48, 1.0));
    let o = h.render(seconds(0.3), &OUT).remove(0);
    assert!(brightness(&o[4_800..]) > bright(0.0, 0.0) * 1.5);
    h.device_mut().reset();
    assert_eq!(
        h.render(2, &OUT)[0].iter().filter(|s| **s != 0.0).count(),
        0
    );
}

#[test]
fn the_lfo_to_index_sweeps_the_brightness() {
    let (mut h, _) = synth();
    patch(&mut h, 0, [0.5, 0.5, 0.0, 0.0], [1.0, 2.0, 1.0, 1.0]);
    h.set_param("index", 0.5);
    h.set_param("lfo.rate", 3.0);
    h.set_param("lfo.to_index", 0.5);
    h.push_note("notes_in", on(0, 48, 1.0));
    let o = h.render(seconds(1.0), &OUT).remove(0);
    let w = 2_400;
    let b: Vec<f32> = (2..20)
        .map(|i| brightness(&o[i * w..(i + 1) * w]))
        .collect();
    let (lo, hi) = b
        .iter()
        .fold((f32::MAX, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
    assert!(hi > lo * 1.6, "{b:?}");
}

#[test]
fn the_level_cv_and_the_pan_cv_shape_the_output() {
    let run = |setup: &dyn Fn(&mut DeviceHarness)| {
        let (mut h, _) = synth();
        pure(&mut h);
        setup(&mut h);
        h.push_note("notes_in", on(0, 57, 1.0));
        let o = h.render(seconds(0.4), &OUT);
        (rms(&o[0][9_600..]), rms(&o[1][9_600..]))
    };
    let (l0, r0) = run(&|_| {});
    assert_eq!(l0.to_bits(), r0.to_bits(), "unconnected: dual mono");
    // The level CV: -1 is 24 dB down when connected.
    let (l, _) = run(&|h| {
        h.set_connected("level_cv_in", true);
        *h.input_mut("level_cv_in") = [-1.0; BATCH];
    });
    assert!((20.0 * (l / l0).log10() + 24.0).abs() < 0.5, "{l} {l0}");
    // The pan CV at +1 silences the left.
    let (l, r) = run(&|h| {
        h.set_connected("pan_cv_in", true);
        *h.input_mut("pan_cv_in") = [1.0; BATCH];
    });
    assert!(l < r0 * 1e-3 && (r - r0).abs() < r0 * 1e-3, "{l} {r}");
}

#[test]
fn spread_pans_the_voices_by_key() {
    let sides = |key: u8, spread: f32| {
        let (mut h, _) = synth();
        pure(&mut h);
        h.set_param("spread", spread);
        h.push_note("notes_in", on(0, key, 1.0));
        let o = h.render(seconds(0.3), &OUT);
        (rms(&o[0][4_800..]), rms(&o[1][4_800..]))
    };
    let (l, r) = sides(60, 1.0);
    assert_eq!(l.to_bits(), r.to_bits(), "middle C is centred");
    let (l, r) = sides(24, 1.0);
    assert!(l > r * 100.0, "a low key is left: {l} {r}");
    let (l, r) = sides(96, 1.0);
    assert!(r > l * 100.0, "a high key is right: {l} {r}");
    let (l, r) = sides(24, 0.0);
    assert_eq!(l.to_bits(), r.to_bits(), "no spread, no pan");
}

// ------------------------------------------------------------------------------- mono modes

fn mono_patch(h: &mut DeviceHarness, mode: f32) {
    pure(h);
    h.set_param("play.mode", mode);
}

fn freq_at(o: &[f32], from: usize) -> f64 {
    dominant_frequency(&o[from..from + 8_192], SR)
}

#[test]
fn mono_plays_one_voice_and_returns_to_the_held_key() {
    for mode in [1.0, 2.0] {
        let (mut h, probe) = synth();
        mono_patch(&mut h, mode);
        h.push_note("notes_in", on(0, 57, 1.0));
        let mut o = h.render(seconds(0.3), &OUT).remove(0);
        h.push_note("notes_in", on(0, 69, 1.0));
        o.extend(h.render(seconds(0.3), &OUT).remove(0));
        assert_eq!(probe.active(), 1, "mode {mode}: one voice");
        assert!(
            (freq_at(&o, 20_000) - 440.0).abs() < 2.0,
            "the last key wins"
        );
        h.push_note("notes_in", off(0, 69));
        o.extend(h.render(seconds(0.3), &OUT).remove(0));
        assert!(
            (freq_at(&o, 30_000) - 220.0).abs() < 2.0,
            "back to the held key"
        );
        assert_eq!(probe.active(), 1);
        h.push_note("notes_in", off(0, 57));
        h.render(seconds(0.5), &OUT);
        assert_eq!(probe.active(), 0, "mode {mode}: released");
    }
}

#[test]
fn releasing_a_key_that_is_not_sounding_changes_nothing() {
    let (mut h, probe) = synth();
    mono_patch(&mut h, 1.0);
    h.push_note("notes_in", on(0, 57, 1.0));
    h.render(seconds(0.1), &OUT);
    h.push_note("notes_in", on(0, 69, 1.0));
    h.render(seconds(0.1), &OUT);
    h.push_note("notes_in", off(0, 57));
    let o = h.render(seconds(0.5), &OUT).remove(0);
    assert!((freq_at(&o, 5_000) - 440.0).abs() < 2.0);
    assert_eq!(probe.keys(), vec![69]);
    // The 57 is gone from the stack: releasing 69 now ends the note.
    h.push_note("notes_in", off(0, 69));
    h.render(seconds(1.0), &OUT);
    assert_eq!(probe.active(), 0);
}

#[test]
fn legato_keeps_the_envelope_running_and_mono_restarts_it() {
    let level_after = |mode: f32| {
        let (mut h, _) = synth();
        mono_patch(&mut h, mode);
        h.set_param("op1.attack", 400.0);
        h.set_param("op1.decay", 50.0);
        h.set_param("op1.sustain", 0.5);
        h.push_note("notes_in", on(0, 57, 1.0));
        h.render(seconds(1.5), &OUT);
        h.push_note("notes_in", on(0, 64, 1.0));
        let o = h.render(seconds(0.3), &OUT).remove(0);
        // The attack of a retrigger climbs from the sustain to the top.
        peak(&o[..6_000])
    };
    let (mono, legato) = (level_after(1.0), level_after(2.0));
    assert!(mono > legato * 1.5, "mono {mono}, legato {legato}");
    assert!(
        (legato - 0.25).abs() < 0.03,
        "legato holds the sustain: {legato}"
    );
}

#[test]
fn the_glide_slides_to_the_new_key() {
    let (mut h, _) = synth();
    mono_patch(&mut h, 1.0);
    h.set_param("play.glide", 150.0);
    h.push_note("notes_in", on(0, 57, 1.0));
    h.render(seconds(0.4), &OUT);
    h.push_note("notes_in", on(0, 69, 1.0));
    let o = h.render(seconds(1.5), &OUT).remove(0);
    let early = dominant_frequency(&o[1_000..3_048], SR);
    assert!(early > 225.0 && early < 400.0, "on its way: {early}");
    assert!((freq_at(&o, 60_000) - 440.0).abs() < 2.0, "arrived");
    // No glide: the new key at once.
    let (mut h, _) = synth();
    mono_patch(&mut h, 1.0);
    h.push_note("notes_in", on(0, 57, 1.0));
    h.render(seconds(0.4), &OUT);
    h.push_note("notes_in", on(0, 69, 1.0));
    let o = h.render(seconds(0.5), &OUT).remove(0);
    assert!((dominant_frequency(&o[1_000..3_048], SR) - 440.0).abs() < 25.0);
}

#[test]
fn switching_between_modes_releases_the_extra_voices() {
    let (mut h, probe) = synth();
    pure(&mut h);
    for (i, key) in [60u8, 64, 67].iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *key, 0.8));
    }
    h.render(seconds(0.1), &OUT);
    assert_eq!(probe.active(), 3);
    h.set_param("play.mode", 1.0);
    h.render(seconds(1.0), &OUT);
    assert_eq!(probe.active(), 1, "only the first voice is left");
}

// ------------------------------------------------------------------------------- aliasing

/// Highest level (dB below the strongest partial) among the bins that are not near a harmonic of
/// `f0`, for a Hann-windowed 32768 samples of `x`. (An FM tone can have a weak fundamental, so
/// the reference is the strongest harmonic.)
fn aliasing_db(x: &[f32], f0: f64) -> f64 {
    let n = 1 << 15;
    let mags = magnitude_spectrum(&x[..n]);
    let bin_hz = SR / n as f64;
    let near_harmonic = |k: usize| {
        let h = (k as f64 * bin_hz / f0).round();
        h >= 1.0 && (k as f64 * bin_hz - h * f0).abs() < 12.0 * bin_hz
    };
    let strongest = mags
        .iter()
        .enumerate()
        .skip(8)
        .filter(|(k, _)| near_harmonic(*k))
        .fold(0.0f64, |m, (_, v)| m.max(*v));
    let worst = mags
        .iter()
        .enumerate()
        .skip(8)
        .filter(|(k, _)| !near_harmonic(*k))
        .fold(0.0f64, |m, (_, v)| m.max(*v));
    20.0 * (worst / strongest).log10()
}

#[test]
fn bright_patches_on_high_notes_do_not_alias() {
    // Everything open: the full index, an octave ratio and feedback, on keys that put the
    // partials far past Nyquist if nothing limited them.
    for key in [84u8, 96, 105, 112] {
        let (mut h, _) = synth();
        patch(&mut h, 0, [0.6, 1.0, 1.0, 1.0], [1.0, 1.0, 2.0, 1.0]);
        h.set_param("feedback", 1.0);
        h.push_note("notes_in", on(0, key, 1.0));
        let o = h.render(seconds(1.0), &OUT).remove(0);
        let alias = aliasing_db(&o[9_600..], hz(key));
        assert!(
            alias < -50.0,
            "key {key} ({:.0} Hz): alias {alias:.1} dB",
            hz(key)
        );
    }
}

#[test]
fn the_alias_meter_catches_the_same_patch_without_the_limiter() {
    // The reference voice has no limiter; on a high key it aliases far above the limit.
    let f0 = hz(96);
    let x = reference(0, [1.0; 4], [0.6, 1.0, 1.0, 1.0], 1.0, 0.0, 96, 48_000);
    assert!(
        aliasing_db(&x[9_600..], f0) > -35.0,
        "{}",
        aliasing_db(&x[9_600..], f0)
    );
}

#[test]
fn the_index_follows_the_pitch_continuously() {
    // A glide up two octaves: no click from the limiter's scale (the step stays small).
    let (mut h, _) = synth();
    patch(&mut h, 0, [0.5, 1.0, 0.0, 0.0], [1.0, 1.0, 1.0, 1.0]);
    h.set_param("pitch.env_amount", -36.0);
    h.set_param("pitch.env_decay", 800.0);
    h.push_note("notes_in", on(0, 96, 1.0));
    let o = h.render(seconds(1.5), &OUT).remove(0);
    assert!(o.iter().all(|s| s.is_finite() && s.abs() <= 0.5));
}

// ------------------------------------------------------------------------------ determinism

#[test]
fn two_renders_are_identical() {
    let render = || {
        let mut h = DeviceHarness::with_options(Box::new(FmSynth::new()), SR, 120.0, 77);
        h.set_param("algorithm", 6.0);
        h.set_param("feedback", 0.5);
        h.set_param("lfo.wave", 4.0);
        h.set_param("lfo.rate", 7.0);
        h.set_param("lfo.to_index", 0.5);
        h.set_param("lfo.to_pitch", 1.0);
        h.set_param("lfo.to_amp", 0.3);
        h.set_param("spread", 0.7);
        for (i, k) in [40u8, 47, 52, 59].iter().enumerate() {
            h.push_note("notes_in", on(i as u16 * 5, *k, 0.7 + 0.05 * i as f32));
        }
        let a = h.render(seconds(0.3), &OUT);
        h.push_note("notes_in", off(10, 47));
        h.push_note("notes_in", on(20, 76, 0.9));
        let b = h.render(seconds(0.3), &OUT);
        (a, b)
    };
    assert_eq!(render(), render());
}

#[test]
fn reset_replays_the_same_audio() {
    let mut h = DeviceHarness::with_options(Box::new(FmSynth::new()), SR, 120.0, 5);
    h.set_param("lfo.wave", 4.0);
    h.set_param("lfo.to_index", 0.5);
    h.set_param("feedback", 0.4);
    let play = |h: &mut DeviceHarness| {
        h.push_note("notes_in", on(0, 55, 0.8));
        h.render(seconds(0.2), &OUT)
    };
    let first = play(&mut h);
    h.device_mut().reset();
    assert_eq!(play(&mut h), first);
}

// ---------------------------------------------------------------------------------- bounds

#[test]
fn every_param_at_min_max_and_default_stays_finite() {
    let (mut h, _) = synth();
    for (i, k) in [36u8, 60, 84, 108].iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *k, 1.0));
    }
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn output_is_bounded_with_everything_open() {
    for (alg, name) in ALGORITHM_NAMES.iter().enumerate() {
        let (mut h, _) = synth();
        patch(&mut h, alg, [1.0; 4], [1.0, 3.0, 5.0, 7.0]);
        h.set_param("feedback", 1.0);
        h.set_param("index", 2.0);
        h.set_param("wheel.to_index", 2.0);
        h.set_param("master.level", 6.0);
        h.set_param("voices", 16.0);
        push(
            &mut h,
            0,
            NoteKind::Cc {
                controller: 1,
                value: 1.0,
            },
        );
        for (i, k) in (36u8..52).enumerate() {
            h.push_note("notes_in", on(i as u16, k, 1.0));
        }
        let o = h.render(seconds(0.5), &OUT);
        let bad = o
            .iter()
            .flatten()
            .find(|s| !(s.is_finite() && s.abs() <= 4.0));
        assert!(bad.is_none(), "{}: {bad:?}", name);
    }
}

#[test]
fn the_default_patch_sounds_and_stays_bounded() {
    let (mut h, _) = synth();
    for (i, k) in [48u8, 55, 60, 64].iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *k, 0.9));
    }
    let o = h.render(seconds(0.5), &OUT);
    assert!(rms(&o[0]) > 0.02);
    assert!(peak(&o[0]) < 1.5, "peak {}", peak(&o[0]));
}

// ------------------------------------------------------------------------------------ sounds

#[test]
fn the_init_sound_is_the_default_patch() {
    let v = sounds::values(0);
    assert_eq!(v.len(), PARAM_COUNT - 1);
    for (key, value) in v {
        let p = DESCRIPTOR.param_by_key(key).unwrap();
        assert_eq!(value, p.default, "{key}");
    }
}

#[test]
fn every_sound_is_complete_and_in_range() {
    assert_eq!(sounds::SOUNDS.len(), sounds::SOUND_NAMES.len());
    for (i, s) in sounds::SOUNDS.iter().enumerate() {
        assert_eq!(s.name, sounds::SOUND_NAMES[i]);
        let mut seen = std::collections::BTreeSet::new();
        for (key, value) in s.changes {
            let p = DESCRIPTOR
                .param_by_key(key)
                .unwrap_or_else(|| panic!("{}: {key} is not a param", s.name));
            assert_ne!(*key, "sound", "{}", s.name);
            assert!(seen.insert(*key), "{}: {key} twice", s.name);
            assert!(
                (p.min..=p.max).contains(value),
                "{}: {key} = {value} outside {}..{}",
                s.name,
                p.min,
                p.max
            );
            if p.discrete {
                assert_eq!(value.round(), *value, "{}: {key} is discrete", s.name);
            }
        }
        let all = sounds::values(i);
        assert_eq!(all.len(), PARAM_COUNT - 1, "{}", s.name);
        assert!(all.iter().all(|(k, _)| *k != "sound"));
    }
    assert_eq!(sounds::values(99).len(), PARAM_COUNT - 1);
}

fn sound_notes(index: usize, key: u8, velocity: f32) -> Vec<f32> {
    let (mut h, _) = synth();
    for (k, v) in sounds::values(index) {
        h.set_param(k, v);
    }
    h.push_note("notes_in", on(0, key, velocity));
    h.render(seconds(1.0), &OUT).remove(0)
}

#[test]
fn every_sound_plays_on_every_register_and_stays_bounded() {
    for (i, s) in sounds::SOUNDS.iter().enumerate() {
        for key in [28u8, 48, 60, 72, 96] {
            let o = sound_notes(i, key, 0.8);
            assert!(
                o.iter().all(|v| v.is_finite() && v.abs() <= 1.5),
                "{} key {key}",
                s.name
            );
            assert!(peak(&o) > 0.02, "{} key {key}: audible", s.name);
        }
    }
}

#[test]
fn the_sounds_differ_from_each_other() {
    let rendered: Vec<Vec<f32>> = (0..sounds::SOUNDS.len())
        .map(|i| sound_notes(i, 60, 0.8))
        .collect();
    for a in 0..rendered.len() {
        for b in a + 1..rendered.len() {
            let d = rms_diff(&rendered[a][2_000..], &rendered[b][2_000..]);
            assert!(
                d > 0.01,
                "{} and {} sound alike ({d})",
                sounds::SOUND_NAMES[a],
                sounds::SOUND_NAMES[b]
            );
        }
    }
}

#[test]
fn the_sounds_do_what_their_names_say() {
    // The bell rings on for seconds and the e-piano's tine dies before its body.
    let bell = sound_notes(2, 72, 0.8);
    assert!(peak(&bell[48_000 - 4_800..]) > 0.0);
    let bell_long = {
        let (mut h, _) = synth();
        for (k, v) in sounds::values(2) {
            h.set_param(k, v);
        }
        h.push_note("notes_in", on(0, 72, 0.8));
        h.render(seconds(1.5), &OUT).remove(0)
    };
    assert!(peak(&bell_long[60_000..]) > 0.01, "the bell rings on");
    let ep = sound_notes(1, 60, 0.9);
    let ratio = |a: &[f32]| brightness(a) / rms(a);
    assert!(
        ratio(&ep[48..1_500]) > ratio(&ep[24_000..28_000]) * 1.5,
        "the tine fades first"
    );
    // The brass is slower to open than the e-piano.
    let brass = sound_notes(4, 60, 0.9);
    assert!(peak(&brass[..240]) < 0.4 * peak(&brass[4_800..9_600]));
    // The pad swells.
    let pad = sound_notes(5, 60, 0.9);
    assert!(peak(&pad[..2_400]) < 0.2 * peak(&pad[40_000..]));
    // The bass is monophonic with a glide.
    let (mut h, probe) = synth();
    for (k, v) in sounds::values(3) {
        h.set_param(k, v);
    }
    for (i, key) in [36u8, 40, 43].iter().enumerate() {
        h.push_note("notes_in", on(i as u16, *key, 0.9));
    }
    h.render(seconds(0.2), &OUT);
    assert_eq!(probe.active(), 1);
}

#[test]
fn master_tune_shifts_the_carrier_by_the_cents_ratio_and_not_a_fixed_operator() {
    for cents in [50.0f32, -100.0, 100.0] {
        let (mut h, _) = synth();
        pure(&mut h);
        h.time_mut().tune_cents = cents;
        h.push_note("notes_in", on(0, 69, 1.0));
        let o = h.render(seconds(1.0), &OUT);
        let f = dominant_frequency(&o[0][4_800..4_800 + 32_768], SR);
        let want = 440.0 * 2f64.powf(f64::from(cents) / 1200.0);
        assert!((f - want).abs() < 0.5, "{cents} cents: {f} vs {want}");
    }
    // A fixed-frequency operator keeps its own Hz.
    let (mut h, _) = synth();
    pure(&mut h);
    h.set_param("op1.fixed", 1.0);
    h.set_param("op1.coarse", 1.0);
    let plain = {
        h.push_note("notes_in", on(0, 69, 1.0));
        let o = h.render(seconds(1.0), &OUT);
        dominant_frequency(&o[0][4_800..4_800 + 32_768], SR)
    };
    let (mut h, _) = synth();
    pure(&mut h);
    h.set_param("op1.fixed", 1.0);
    h.set_param("op1.coarse", 1.0);
    h.time_mut().tune_cents = 100.0;
    h.push_note("notes_in", on(0, 69, 1.0));
    let o = h.render(seconds(1.0), &OUT);
    let tuned = dominant_frequency(&o[0][4_800..4_800 + 32_768], SR);
    assert!((plain - tuned).abs() < 0.5, "{plain} vs {tuned}");
}

#[test]
fn a_tune_of_zero_renders_the_same_bits_as_no_tune() {
    let run = |set: bool| {
        let (mut h, _) = synth();
        pure(&mut h);
        if set {
            h.time_mut().tune_cents = 0.0;
        }
        h.push_note("notes_in", on(0, 60, 1.0));
        h.render(seconds(0.3), &OUT)
    };
    let (a, b) = (run(false), run(true));
    assert!(
        a[0].iter()
            .zip(&b[0])
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
}
