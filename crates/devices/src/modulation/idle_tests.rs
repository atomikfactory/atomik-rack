//! The modulation effect says it is quiescent only when a silent batch would change nothing but
//! its free-running LFO phase, and a skipped stretch leaves the phase where processing it would:
//! the sweep is heard the moment sound returns, so every mode is run with the depth up.

use super::*;
use crate::test_support::{IdleRun, harness, skip_matches_process};
use atomik_testkit::harness::DeviceHarness;

fn noise(state: &mut u32) -> [f32; BATCH] {
    let mut b = [0.0; BATCH];
    for x in &mut b {
        *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *x = (*state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
    }
    b
}

fn feed(h: &mut DeviceHarness, l: [f32; BATCH], r: [f32; BATCH]) {
    *h.input_mut("in_l") = l;
    *h.input_mut("in_r") = r;
    h.process();
}

fn quiet(h: &DeviceHarness) -> bool {
    h.device().quiescent()
}

/// A modulation effect of `mode` with the sweep as deep as it goes and some feedback.
fn make(mode: f32) -> DeviceHarness {
    let mut h = harness(Box::new(Modulation::new()));
    for (k, v) in [
        ("mode", mode),
        ("rate", 3.7),
        ("depth", 1.0),
        ("feedback", 0.5),
        ("delay", 12.0),
        ("spread", 90.0),
        ("mix", 0.6),
        ("stages", 8.0),
    ] {
        h.set_param(k, v);
    }
    h
}

const MODES: [(f32, &str); 3] = [(0.0, "chorus"), (1.0, "flanger"), (2.0, "phaser")];

#[test]
fn it_is_quiescent_only_after_what_it_holds_has_run_dry() {
    for (mode, name) in MODES {
        let mut h = make(mode);
        assert!(!quiet(&h), "{name}: nothing has run");
        let silence = [0.0; BATCH];
        feed(&mut h, silence, silence);
        assert!(quiet(&h), "{name}: a fresh effect holds nothing");
        let mut s = 1;
        for _ in 0..20 {
            feed(&mut h, noise(&mut s), noise(&mut s));
            assert!(!quiet(&h), "{name}: sound");
        }
        let mut batches = 0;
        while !quiet(&h) {
            feed(&mut h, silence, silence);
            batches += 1;
            assert!(batches < 3_000, "{name}: it never settled");
        }
        if mode < 2.0 {
            // The ring of the lines is about 2,760 samples: 43 batches of zeros.
            assert!(batches >= 40, "{name}: {batches} batches");
        }
        h.set_param("depth", 0.4);
        assert!(!quiet(&h), "{name}: a param change is heard first");
        feed(&mut h, silence, silence);
        assert!(!quiet(&h), "{name}: the depth control has not arrived");
        for _ in 0..200 {
            feed(&mut h, silence, silence);
        }
        assert!(quiet(&h), "{name}");
        h.set_connected("mix_cv_in", true);
        feed(&mut h, silence, silence);
        assert!(!quiet(&h), "{name}: a connected mix CV is read every batch");
    }
}

#[test]
fn an_lfo_locked_to_a_playing_transport_never_rests_and_a_stopped_one_does() {
    for (mode, name) in MODES {
        let mut h = make(mode);
        h.set_param("sync", 3.0);
        h.time_mut().playing = true;
        let silence = [0.0; BATCH];
        for _ in 0..600 {
            feed(&mut h, silence, silence);
            assert!(!quiet(&h), "{name}: locked");
        }
        h.time_mut().playing = false;
        feed(&mut h, silence, silence);
        assert!(quiet(&h), "{name}: free-running at the synced rate");
    }
}

/// One effect processed through the silence, one skipped through it, with a parameter change, a
/// tempo change (the LFO is synced to the stopped transport, so the tempo is its rate) and sound
/// again after it.
#[test]
fn skipping_each_mode_in_silence_leaves_what_processing_the_silence_would() {
    for (mode, name) in MODES {
        for synced in [false, true] {
            let skipped = skip_matches_process(
                &|| {
                    let mut h = make(mode);
                    if synced {
                        h.set_param("sync", 2.0);
                        // Stopped: the LFO free-runs at the synced rate.
                        h.time_mut().playing = false;
                    }
                    h
                },
                &IdleRun {
                    ins: &["in_l", "in_r"],
                    outs: &["out_l", "out_r"],
                    batches: 6_000,
                    sound: &|n| n < 30 || (4_000..4_040).contains(&n),
                    edit: &|n, h| match n {
                        1_500 => {
                            h.set_param("feedback", 0.3);
                            true
                        }
                        2_500 => {
                            h.time_mut().tempo = atomik_core::Tempo::new(133.0).unwrap();
                            true
                        }
                        3_000 => {
                            h.set_param("rate", 9.1);
                            true
                        }
                        _ => false,
                    },
                },
            );
            assert!(
                skipped > 2_000,
                "{name} synced {synced}: the scenario skips a lot: {skipped}"
            );
        }
    }
}

/// Locked to a playing transport the effect is processed in every batch, and the render is of
/// course the same.
#[test]
fn a_locked_lfo_is_processed_through_the_silence() {
    let skipped = skip_matches_process(
        &|| {
            let mut h = make(0.0);
            h.set_param("sync", 2.0);
            h.time_mut().playing = true;
            h
        },
        &IdleRun {
            ins: &["in_l", "in_r"],
            outs: &["out_l", "out_r"],
            batches: 1_500,
            sound: &|n| n < 30 || (1_000..1_040).contains(&n),
            edit: &|_, _| false,
        },
    );
    assert_eq!(skipped, 0);
}

/// A switch of mode in the silence clears the state; the effect rests again afterwards and the
/// first sound in the new mode is the same bits.
#[test]
fn a_change_of_mode_in_the_silence_is_the_same_bits() {
    let skipped = skip_matches_process(
        &|| make(0.0),
        &IdleRun {
            ins: &["in_l", "in_r"],
            outs: &["out_l", "out_r"],
            batches: 6_000,
            sound: &|n| n < 30 || (4_000..4_040).contains(&n),
            edit: &|n, h| match n {
                1_000 => {
                    h.set_param("mode", 2.0);
                    true
                }
                2_000 => {
                    h.set_param("mode", 1.0);
                    true
                }
                _ => false,
            },
        },
    );
    assert!(skipped > 2_000, "{skipped}");
}
