//! The poly synth's appended params all default to nothing: a busy patch (both oscillators, noise,
//! both envelopes, the LFO to pitch and cutoff, resonance, CV, a voice steal and a release)
//! renders to the same bits it did before the oscillator, filter, LFO, envelope, play, spread and
//! effect params were appended. The hash was taken from the synth before they existed.
//!
//! The exact bits depend on the platform maths library, so like the render goldens it runs only
//! on the platform the hash was taken on.
#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

atomik_testkit::install_alloc_guard!();

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_devices::poly_synth::PolySynth;
use atomik_testkit::harness::DeviceHarness;

/// FNV-1a over the bits of every sample.
fn fnv(bits: impl Iterator<Item = u32>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bits {
        for byte in b.to_le_bytes() {
            h = (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn busy_patch() -> DeviceHarness {
    let mut h = DeviceHarness::with_options(Box::new(PolySynth::new()), 48_000.0, 120.0, 11);
    for (k, v) in [
        ("osc1.wave", 0.0),
        ("osc2.wave", 1.0),
        ("osc2.octave", -1.0),
        ("osc.mix", 0.4),
        ("noise.level", 0.05),
        ("filter.cutoff", 900.0),
        ("filter.resonance", 0.6),
        ("filter.env_amount", 0.7),
        ("filter.key_track", 0.5),
        ("lfo.wave", 4.0),
        ("lfo.rate", 5.0),
        ("lfo.to_pitch", 0.2),
        ("lfo.to_cutoff", 0.7),
        ("env.amp.attack", 8.0),
        ("env.amp.release", 120.0),
        ("voices", 3.0),
    ] {
        h.set_param(k, v);
    }
    *h.input_mut("cutoff_cv_in") = [0.3; BATCH];
    *h.input_mut("resonance_cv_in") = [0.2; BATCH];
    h
}

#[test]
fn the_defaults_of_the_appended_params_render_the_old_bits() {
    let mut h = busy_patch();
    let on = |offset, key, velocity| NoteEvent {
        offset,
        kind: NoteKind::On { key, velocity },
    };
    let mut bits = Vec::new();
    for batch in 0..400usize {
        match batch {
            0 => {
                h.push_note("notes_in", on(0, 45, 0.9));
                h.push_note("notes_in", on(10, 52, 0.7));
            }
            20 => {
                h.push_note("notes_in", on(3, 59, 0.8));
                h.push_note("notes_in", on(30, 64, 1.0));
            }
            90 => h.push_note(
                "notes_in",
                NoteEvent {
                    offset: 5,
                    kind: NoteKind::Off { key: 52 },
                },
            ),
            200 => h.push_note("notes_in", on(1, 71, 0.5)),
            _ => {}
        }
        let out = h.render(1, &["out_l", "out_r"]);
        bits.extend(out.iter().flatten().map(|s| s.to_bits()));
    }
    assert_eq!(
        fnv(bits.into_iter()),
        0xa289_294c_9751_b461,
        "the render changed: a default of an appended param is not neutral"
    );
}
