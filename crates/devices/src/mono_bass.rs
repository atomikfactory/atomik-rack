//! `atomik.mono-bass`: a monophonic bass synthesizer on the poly synth's voice.
//!
//! It is the poly synth ([`crate::poly_synth`]) with another set of defaults and the same
//! params, sockets and sound engine, so everything the voice does is documented there. What makes
//! it a bass:
//!
//! * **One voice, legato, glide.** `voices` is 1 and `legato` is on: a key played while another is
//!   held changes the pitch only (the envelopes run on, a key released returns to the key still
//!   held), and `glide` slides the pitch between keys (60 ms to cover most of the way).
//! * **Ladder filter.** `filter.type` is the 24 dB ladder, with `filter.drive` clipping in front
//!   of it (anti-aliased) and a resonance that rings without ever self-oscillating.
//! * **Sound.** A saw and a square an octave down, a short filter envelope with a low sustain,
//!   a short release, and a light chorus (`chorus.*`); `delay.*` adds an echo.
//!
//! Nothing else differs: it takes every param of the poly synth, so a patch of one carries over
//! to the other by key.

use atomik_core::ParamDescriptor;
use atomik_device_sdk::{DeviceDescriptor, DeviceFactory, DeviceKind};

use crate::poly_synth::{
    BASE_PARAMS, P_AMP_A, P_AMP_D, P_AMP_R, P_AMP_S, P_CHORUS_MIX, P_CUTOFF, P_DRIVE, P_ENV_AMOUNT,
    P_FILTER_TYPE, P_FLT_D, P_FLT_S, P_GLIDE, P_KEY_TRACK, P_LEGATO, P_OSC_MIX, P_OSC2_FINE,
    P_OSC2_OCTAVE, P_RESONANCE, P_VOICES, PARAM_COUNT, PolySynth, SOCKETS,
};

/// `base` with the defaults of `set` (param id, default) replaced.
const fn with_defaults(
    mut base: [ParamDescriptor; PARAM_COUNT],
    set: &[(u32, f32)],
) -> [ParamDescriptor; PARAM_COUNT] {
    let mut i = 0;
    while i < set.len() {
        base[set[i].0 as usize].default = set[i].1;
        i += 1;
    }
    base
}

static PARAMS: [ParamDescriptor; PARAM_COUNT] = with_defaults(
    BASE_PARAMS,
    &[
        (P_OSC2_OCTAVE, -1.0),
        (P_OSC2_FINE, 0.0),
        (P_OSC_MIX, 0.4),
        (P_CUTOFF, 450.0),
        (P_RESONANCE, 0.35),
        (P_ENV_AMOUNT, 0.6),
        (P_KEY_TRACK, 0.35),
        (P_AMP_A, 1.0),
        (P_AMP_D, 250.0),
        (P_AMP_S, 0.8),
        (P_AMP_R, 90.0),
        (P_FLT_D, 220.0),
        (P_FLT_S, 0.1),
        (P_VOICES, 1.0),
        (P_FILTER_TYPE, 1.0),
        (P_DRIVE, 0.25),
        (P_GLIDE, 60.0),
        (P_LEGATO, 1.0),
        (P_CHORUS_MIX, 0.15),
    ],
);

/// Static description of the mono bass.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.mono-bass",
    name: "Mono Bass",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering the mono bass.
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(create()),
    }
}

/// A mono bass with its default params (call `prepare` before processing): the poly synth's
/// engine under this device's descriptor. Builds the shared wave bank on the first call in the
/// process, so create it off the audio thread.
pub fn create() -> PolySynth {
    PolySynth::with_descriptor(&DESCRIPTOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{SR, assert_keys, harness, off, on, sweep_params_min_max};
    use atomik_core::{BATCH, NoteEvent, NoteKind};
    use atomik_testkit::harness::DeviceHarness;
    use atomik_testkit::signal::{dominant_frequency, max_step, peak, rms};

    const OUT: [&str; 2] = ["out_l", "out_r"];

    fn seconds(s: f64) -> usize {
        (s * SR / BATCH as f64).ceil() as usize
    }

    fn bass() -> DeviceHarness {
        harness(Box::new(create()))
    }

    /// A single sine, the filter wide open.
    fn sine(h: &mut DeviceHarness) {
        h.set_param("osc1.wave", 3.0);
        h.set_param("osc.mix", 0.0);
        h.set_param("filter.cutoff", 20_000.0);
        h.set_param("filter.env_amount", 0.0);
        h.set_param("filter.key_track", 0.0);
        h.set_param("filter.resonance", 0.0);
        h.set_param("filter.drive", 0.0);
        h.set_param("chorus.mix", 0.0);
    }

    #[test]
    fn the_descriptor_is_the_poly_synths_with_bass_defaults() {
        assert_eq!(DESCRIPTOR.validate(), Ok(()));
        assert_eq!(DESCRIPTOR.type_id, "atomik.mono-bass");
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
        let poly = &crate::poly_synth::DESCRIPTOR;
        let keys = |d: &DeviceDescriptor| d.params.iter().map(|p| p.key).collect::<Vec<_>>();
        assert_eq!(keys(&DESCRIPTOR), keys(poly));
        let d = |k: &str| DESCRIPTOR.param_by_key(k).unwrap().default;
        assert_eq!(
            (d("voices"), d("legato"), d("filter.type")),
            (1.0, 1.0, 1.0)
        );
        assert!((d("glide") - 60.0).abs() < 1e-6 && d("filter.drive") > 0.0);
        // Only the defaults differ: ranges and tapers are the poly synth's.
        for (a, b) in DESCRIPTOR.params.iter().zip(poly.params) {
            assert_eq!(
                (a.min, a.max, a.taper, a.discrete),
                (b.min, b.max, b.taper, b.discrete)
            );
        }
        assert_keys(
            &DESCRIPTOR,
            &keys(&DESCRIPTOR),
            &[
                "notes_in",
                "cutoff_cv_in",
                "pitch_cv_in",
                "resonance_cv_in",
                "level_cv_in",
                "pan_cv_in",
                "out_l",
                "out_r",
                "lfo_cv_out",
            ],
        );
    }

    #[test]
    fn it_sounds_bounded_and_stays_one_voice() {
        let mut h = bass();
        for (i, key) in [36u8, 40, 43].iter().enumerate() {
            h.push_note("notes_in", on(i as u16 * 8, *key, 0.9));
        }
        let o = h.render(seconds(1.0), &OUT);
        assert!(rms(&o[0]) > 0.02, "{}", rms(&o[0]));
        assert!(peak(&o[0]) < 4.0 && o[0].iter().all(|s| s.is_finite()));
        // The default chorus makes the pair differ slightly; with it off the output is mono.
        assert_ne!(o[0], o[1]);
        h.set_param("chorus.mix", 0.0);
        h.push_note("notes_in", on(0, 36, 0.9));
        let o = h.render(seconds(0.3), &OUT);
        assert_eq!(o[0], o[1], "mono output");
    }

    /// Frequency of `o` over the sample range `r`.
    fn hz(o: &[f32], r: std::ops::Range<usize>) -> f64 {
        dominant_frequency(&o[r], SR)
    }

    #[test]
    fn a_legato_key_changes_the_pitch_without_retriggering() {
        let mut h = bass();
        sine(&mut h);
        h.set_param("glide", 0.0);
        h.set_param("env.amp.attack", 400.0);
        h.set_param("env.amp.sustain", 1.0);
        h.set_param("osc1.octave", 0.0);
        h.push_note("notes_in", on(0, 45, 1.0)); // 110 Hz
        let mut out = h.render(seconds(0.6), &OUT).remove(0);
        // The attack has run its course. A second key while the first is held: new pitch, the
        // level continues (no restart of the 400 ms attack), no click.
        h.push_note("notes_in", on(0, 57, 1.0)); // 220 Hz
        out.extend(h.render(seconds(0.3), &OUT).remove(0));
        let n = out.len();
        assert!((hz(&out, 4_800 + 14_400..28_800) - 110.0).abs() < 2.0);
        assert!((hz(&out, n - 9_600..n) - 220.0).abs() < 3.0);
        let before = rms(&out[28_800 - 4_800..28_800]);
        let after = rms(&out[28_800 + 480..28_800 + 5_280]);
        assert!(
            (after / before - 1.0).abs() < 0.1,
            "level continues: {before} -> {after}"
        );
        assert!(max_step(&out) < 0.1, "click: {}", max_step(&out));
    }

    #[test]
    fn releasing_the_top_key_returns_to_the_one_still_held() {
        let mut h = bass();
        sine(&mut h);
        h.set_param("glide", 0.0);
        h.push_note("notes_in", on(0, 45, 1.0));
        h.push_note("notes_in", on(40, 57, 1.0));
        let mut out = h.render(seconds(0.4), &OUT).remove(0);
        h.push_note("notes_in", off(0, 57));
        out.extend(h.render(seconds(0.4), &OUT).remove(0));
        let n = out.len();
        assert!((hz(&out, 4_800..19_200) - 220.0).abs() < 3.0);
        assert!(
            (hz(&out, n - 9_600..n) - 110.0).abs() < 2.0,
            "back to 110 Hz"
        );
        // Releasing the last key releases the voice.
        h.push_note("notes_in", off(0, 45));
        let tail = h.render(seconds(0.5), &OUT).remove(0);
        assert!(rms(&tail[tail.len() - 4_800..]) < 1e-4);
    }

    #[test]
    fn glide_covers_most_of_the_way_in_its_time_constant() {
        // 100 ms glide from 110 Hz to 220 Hz: after one time constant the pitch has covered
        // 1 - 1/e of the octave (in semitones), after five it has arrived.
        let mut h = bass();
        sine(&mut h);
        h.set_param("glide", 100.0);
        h.push_note("notes_in", on(0, 45, 1.0));
        h.render(seconds(0.5), &OUT);
        h.push_note("notes_in", on(0, 57, 1.0));
        let o = h.render(seconds(1.0), &OUT).remove(0);
        // Instantaneous pitch from the zero crossings in short windows.
        let at = |centre: usize| hz(&o, centre - 1_200..centre + 1_200);
        let semis = |f: f64| 12.0 * (f / 110.0).log2();
        let tau = 4_800;
        let s1 = semis(at(tau));
        assert!((s1 - 12.0 * (1.0 - (-1.0f64).exp())).abs() < 1.2, "{s1}");
        let s5 = semis(at(5 * tau));
        assert!((s5 - 12.0).abs() < 0.3, "{s5}");
        // Glide never jumps: the pitch is monotone up.
        assert!(at(2 * tau) > at(tau) && at(3 * tau) > at(2 * tau));
    }

    #[test]
    fn the_first_key_after_silence_glides_from_the_last_one() {
        let mut h = bass();
        sine(&mut h);
        h.set_param("glide", 80.0);
        h.set_param("env.amp.release", 20.0);
        h.push_note("notes_in", on(0, 45, 1.0));
        h.render(seconds(0.3), &OUT);
        h.push_note("notes_in", off(0, 45));
        h.render(seconds(0.3), &OUT);
        h.push_note("notes_in", on(0, 57, 1.0));
        let o = h.render(seconds(0.6), &OUT).remove(0);
        let early = hz(&o, 480..2_880);
        let late = hz(&o, 19_200..28_800);
        assert!(
            early < 200.0 && (late - 220.0).abs() < 3.0,
            "{early} {late}"
        );
    }

    #[test]
    fn every_param_min_max_stays_finite() {
        let mut h = bass();
        for k in [24u8, 36, 60, 96] {
            h.push_note("notes_in", on(0, k, 1.0));
        }
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }

    #[test]
    fn it_is_allocation_free_and_deterministic() {
        let run = |seed: u64| {
            let mut h = DeviceHarness::with_options(Box::new(create()), SR, 120.0, seed);
            h.set_param("delay.mix", 0.3);
            for i in 0..200u32 {
                let key = 30 + (i * 5 % 30) as u8;
                h.push_note("notes_in", on((i % 50) as u16, key, 0.8));
                if i % 3 == 0 {
                    h.push_note("notes_in", off(60, key));
                }
                if i % 70 == 0 {
                    h.push_note(
                        "notes_in",
                        NoteEvent {
                            offset: 63,
                            kind: NoteKind::AllOff,
                        },
                    );
                }
                h.process();
            }
            h.render(10, &OUT).remove(0)
        };
        assert_eq!(run(2), run(2));
    }
}
