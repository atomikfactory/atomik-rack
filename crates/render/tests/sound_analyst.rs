//! The sound analyst's patch on synthetic targets: a saw pluck, a square pad, a noise hat and an
//! inharmonic bell give the expected synth family and envelope, and the render of the proposal
//! lands near the target on the three curves.

use atomik_dsp::sound_analysis::{HarmonicKind, SoundProfile, analyse};
use atomik_render::default_registry;
use atomik_render::sound_analyst::{Family, Fit, POLY, fit, measure, propose};
use std::f64::consts::TAU;

const RATE: u32 = 44_100;

fn frames(seconds: f64) -> usize {
    (seconds * f64::from(RATE)) as usize
}

/// The first `harmonics` multiples of `hz` (all or odd only) at `1/k`, times `env(t)`, with the
/// harmonic `k` scaled by `tilt(k, t)`.
fn tone(
    hz: f64,
    (harmonics, odd_only): (usize, bool),
    seconds: f64,
    env: impl Fn(f64) -> f64,
    tilt: impl Fn(usize, f64) -> f64,
) -> Vec<f32> {
    (0..frames(seconds))
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let sum: f64 = (1..=harmonics)
                .filter(|k| !odd_only || k % 2 == 1)
                .map(|k| (TAU * hz * k as f64 * t).sin() * tilt(k, t) / k as f64)
                .sum();
            (0.4 * env(t) * sum) as f32
        })
        .collect()
}

/// A saw pluck at 220 Hz: a 2 ms attack and a fall to a tenth in 0.3 s, duller as it goes.
fn saw_pluck() -> Vec<f32> {
    let tau = 0.3 / 10f64.ln();
    tone(
        220.0,
        (40, false),
        1.5,
        |t| (t / 0.002).min(1.0) * (-t / tau).exp(),
        |k, t| (-((k - 1) as f64) * t / 0.25).exp(),
    )
}

/// A square pad at 220 Hz: a linear attack of 0.3 s, held to 1.5 s, a release of a tenth in 0.23 s.
fn square_pad() -> Vec<f32> {
    tone(
        220.0,
        (39, true),
        2.6,
        |t| {
            if t < 0.3 {
                t / 0.3
            } else if t < 1.5 {
                1.0
            } else {
                (-(t - 1.5) / 0.1).exp()
            }
        },
        |_, _| 1.0,
    )
}

/// White noise through a first difference that falls to a tenth in 60 ms.
fn noise_hat() -> Vec<f32> {
    let mut state = 0x1234_5678_u32;
    let mut last = 0.0f32;
    let tau = 0.06 / 10f64.ln();
    (0..frames(0.5))
        .map(|i| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (state >> 8) as f32 / (1 << 23) as f32 - 1.0;
            let y = x - last;
            last = x;
            let t = i as f64 / f64::from(RATE);
            (0.4 * f64::from(y) * (-t / tau).exp()) as f32
        })
        .collect()
}

/// An inharmonic bell at 220 Hz: partials at 1, 2.32, 3.7 and 6.1 times the pitch, each ringing
/// for about a second.
fn rust_bell() -> Vec<f32> {
    (0..frames(1.5))
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let sum: f64 = [
                (1.0, 1.0, 0.9),
                (2.32, 0.7, 0.6),
                (3.7, 0.5, 0.45),
                (6.1, 0.35, 0.3),
            ]
            .iter()
            .map(|(r, a, tau)| a * (-t / tau).exp() * (TAU * 220.0 * r * t).sin())
            .sum();
            (0.25 * sum) as f32
        })
        .collect()
}

fn profile(x: &[f32]) -> SoundProfile {
    analyse(&[x], RATE).expect("analyses")
}

/// A param that the proposal sets must be a param of the device, within its range.
fn fits_the_device(f: &Fit) {
    let registry = default_registry();
    let desc = registry
        .descriptor(f.proposal.type_id())
        .expect("the synth exists");
    for (key, value) in &f.proposal.params {
        let p = desc
            .params
            .iter()
            .find(|p| p.key == key)
            .unwrap_or_else(|| panic!("{} has no param {key}", desc.name));
        assert!(
            (p.min..=p.max).contains(value),
            "{key} = {value} is outside {}..{}",
            p.min,
            p.max
        );
    }
}

fn within(value: f32, expected: f32, share: f32) -> bool {
    (value - expected).abs() <= expected.abs() * share
}

#[test]
fn a_saw_pluck_gets_a_saw_on_the_poly_synth_with_a_pluck_envelope() {
    let target = profile(&saw_pluck());
    assert_eq!(target.harmonics.kind, HarmonicKind::All);
    let f = fit(&target, &default_registry()).expect("fits");
    fits_the_device(&f);
    let p = &f.proposal;
    assert_eq!((p.family, p.type_id()), (Family::Poly, POLY));
    assert_eq!(p.key, 57, "the note of 220 Hz");
    assert_eq!(p.param("osc1.wave"), Some(0.0), "a saw");
    assert_eq!(p.param("env.amp.sustain"), Some(0.0));
    // A fall to a tenth in 0.3 s is a decay of 0.3 / 0.332 s on this envelope (within a quarter).
    let decay = p.param("env.amp.decay").unwrap();
    assert!(within(decay, 904.0, 0.25), "decay {decay}");
    assert!(p.param("env.amp.attack").unwrap() <= 5.0);
    // It gets duller as it goes: the filter envelope opens the cutoff.
    assert!(
        p.param("filter.env_amount").unwrap() > 0.1,
        "{:?}",
        p.params
    );
    // Heard on its note, the patch is the same pluck.
    let d = f.distances;
    assert!(d.envelope_db < 3.0, "{d:?}");
    assert!(d.brightness_octaves < 0.5, "{d:?}");
    assert!(d.pitch_semitones.unwrap() < 0.1, "{d:?}");
    assert!(within(
        f.rendered.envelope.decay_s,
        target.envelope.decay_s,
        0.3
    ));
}

#[test]
fn a_square_pad_gets_a_square_with_a_slow_attack_a_sustain_and_a_release() {
    let target = profile(&square_pad());
    let f = fit(&target, &default_registry()).expect("fits");
    fits_the_device(&f);
    let p = &f.proposal;
    assert_eq!(p.family, Family::Poly);
    assert_eq!(p.param("osc1.wave"), Some(1.0), "a square");
    // The linear rise of 0.3 s takes 0.24 s from a tenth to nine tenths: 0.24 / 0.748 s here.
    let attack = p.param("env.amp.attack").unwrap();
    assert!(within(attack, 321.0, 0.25), "attack {attack}");
    assert!(p.param("env.amp.sustain").unwrap() >= 0.95);
    // A release of a tenth in 0.23 s is 0.23 / 0.332 s of this envelope.
    let release = p.param("env.amp.release").unwrap();
    assert!(within(release, 693.0, 0.25), "release {release}");
    // The note is held to the end of the plateau.
    assert!((p.hold_s - 1.5).abs() < 0.12, "{}", p.hold_s);
    // A steady brightness has no filter envelope.
    assert_eq!(p.param("filter.env_amount"), Some(0.0));
    let d = f.distances;
    assert!(d.envelope_db < 2.0, "{d:?}");
    assert!(d.brightness_octaves < 0.3, "{d:?}");
    assert!(d.pitch_semitones.unwrap() < 0.1, "{d:?}");
    assert!(within(
        f.rendered.envelope.attack_s,
        target.envelope.attack_s,
        0.2
    ));
    assert!(within(
        f.rendered.envelope.hold_s,
        target.envelope.hold_s,
        0.1
    ));
}

#[test]
fn a_noise_hat_gets_the_noise_voice_with_a_high_pass_filter_and_a_short_decay() {
    let target = profile(&noise_hat());
    let f = fit(&target, &default_registry()).expect("fits");
    fits_the_device(&f);
    let p = &f.proposal;
    assert_eq!(p.family, Family::Noise);
    assert_eq!(p.type_id(), POLY, "the noise voice is the poly synth");
    assert_eq!(p.param("noise.level"), Some(1.0));
    assert_eq!(p.param("filter.mode"), Some(2.0), "a high-pass");
    assert_eq!(p.key, 60, "an unpitched sound plays the middle key");
    // A fall to a tenth in 60 ms.
    let decay = p.param("env.amp.decay").unwrap();
    assert!(within(decay, 181.0, 0.3), "decay {decay}");
    let d = f.distances;
    assert!(d.envelope_db < 4.0, "{d:?}");
    assert!(d.brightness_octaves < 0.5, "{d:?}");
    assert_eq!(
        d.pitch_semitones, None,
        "an unpitched target has no pitch curve"
    );
    assert!(f.rendered.pitch.is_none());
}

#[test]
fn an_inharmonic_bell_gets_the_fm_synth_with_a_modulator_off_the_harmonics() {
    let target = profile(&rust_bell());
    assert_eq!(
        target.harmonics.kind,
        HarmonicKind::Inharmonic,
        "{:?}",
        target.harmonics
    );
    let f = fit(&target, &default_registry()).expect("fits");
    fits_the_device(&f);
    let p = &f.proposal;
    assert_eq!(p.family, Family::Fm);
    assert_eq!(p.type_id(), "atomik.fm-synth");
    let ratio = p.param("op2.coarse").unwrap() + p.param("op2.fine").unwrap();
    assert!(
        [2.32, 3.7, 6.1].iter().any(|r| (ratio - r).abs() < 0.15),
        "the modulator is at {ratio}"
    );
    assert_eq!(p.param("op3.level"), Some(0.0));
    // It rings: a long decay, no sustain.
    assert_eq!(p.param("op1.sustain"), Some(0.0));
    assert!(p.param("op1.decay").unwrap() > 500.0);
    assert!(
        f.rendered.envelope.decay_s > 0.3,
        "{:?}",
        f.rendered.envelope
    );
}

#[test]
fn a_pitch_that_falls_at_the_start_gets_a_pitch_envelope() {
    // A sine at 110 Hz that starts an octave higher and falls to it in about 80 ms.
    let mut phase = 0.0f64;
    let kick: Vec<f32> = (0..frames(0.8))
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let semis = 12.0 * (-t / 0.04).exp();
            phase += TAU * 110.0 * 2f64.powf(semis / 12.0) / f64::from(RATE);
            (0.5 * phase.sin() * (-t / 0.35).exp()) as f32
        })
        .collect();
    let target = profile(&kick);
    let f = fit(&target, &default_registry()).expect("fits");
    fits_the_device(&f);
    let p = &f.proposal;
    assert_eq!(p.param("osc1.wave"), Some(3.0), "a sine");
    assert_eq!(
        p.param("env.mod.dest"),
        Some(0.0),
        "the mod envelope moves the pitch"
    );
    assert!(p.param("env.mod.amount").unwrap() > 0.1, "{:?}", p.params);
    let rendered = f.rendered.pitch.expect("the render is pitched");
    assert!(rendered.start_semitones > 2.0, "{rendered:?}");
}

#[test]
fn the_proposal_is_the_same_every_time_and_the_render_is_deterministic() {
    let target = profile(&saw_pluck());
    assert_eq!(propose(&target), propose(&target));
    let registry = default_registry();
    let a = measure(&propose(&target), &registry).unwrap();
    let b = measure(&propose(&target), &registry).unwrap();
    assert_eq!(a, b);
}

#[test]
fn every_choice_of_the_proposal_is_explained() {
    let f = fit(&profile(&square_pad()), &default_registry()).unwrap();
    let text = f.proposal.reasons.join("\n");
    assert!(text.contains("square"), "{text}");
    assert!(text.contains("Envelope"), "{text}");
    assert!(text.contains("Filter"), "{text}");
}

/// A saw chord of the three keys at once, falling to a tenth in 0.4 s.
fn saw_chord(keys: &[u8]) -> Vec<f32> {
    let tau = 0.4 / 10f64.ln();
    let hz = |key: u8| 440.0 * ((f64::from(key) - 69.0) / 12.0).exp2();
    let parts: Vec<Vec<f32>> = keys
        .iter()
        .map(|k| {
            tone(
                hz(*k),
                (12, false),
                1.4,
                |t| (t / 0.002).min(1.0) * (-t / tau).exp(),
                |_, _| 1.0,
            )
        })
        .collect();
    (0..parts[0].len())
        .map(|i| parts.iter().map(|p| p[i]).sum::<f32>() * 0.5)
        .collect()
}

#[test]
fn a_chord_gets_the_poly_synth_playing_every_note_and_renders_as_that_chord() {
    use atomik_dsp::harmony::detect_sound_chord;
    use atomik_render::sound_analyst::{fit_chord, patch_json};
    let x = saw_chord(&[57, 60, 64]);
    let profile = analyse(&[&x], RATE).unwrap();
    let chord = detect_sound_chord(&[&x], RATE).expect("a chord");
    assert_eq!(chord.keys, [57, 60, 64]);
    let fit = fit_chord(&profile, Some(&chord), &default_registry()).unwrap();
    assert_eq!(fit.proposal.family, Family::Poly);
    assert_eq!(fit.proposal.keys(), [57, 60, 64]);
    assert!(fit.proposal.param("voices").is_some_and(|v| v >= 3.0));
    let json: serde_json::Value = serde_json::from_str(&patch_json(&fit.proposal)).unwrap();
    let played: Vec<u64> = json["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["key"].as_u64().unwrap())
        .collect();
    assert_eq!(played, [57, 60, 64]);
    // The render is the same chord.
    let rendered = measure(&fit.proposal, &default_registry()).unwrap();
    let planes = render_of(&fit.proposal);
    let again = detect_sound_chord(&[&planes], 48_000).expect("the render is a chord too");
    assert_eq!(again.keys, [57, 60, 64]);
    assert!(rendered.duration_s > 0.2);
    // One note takes the single-note path as before.
    let one = propose(&analyse(&[&saw_pluck()], RATE).unwrap());
    assert_eq!(one.keys(), [one.key]);
    assert!(one.chord.is_none());
}

/// The mono samples of the render of `proposal`.
fn render_of(proposal: &atomik_render::sound_analyst::Proposal) -> Vec<f32> {
    let r = atomik_render::render_json(
        &atomik_render::sound_analyst::patch_json(proposal),
        &default_registry(),
        std::num::NonZeroUsize::new(512).unwrap(),
    )
    .unwrap();
    r.channels[0].clone()
}
