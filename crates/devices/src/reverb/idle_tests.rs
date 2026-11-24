//! The reverb says it is quiescent only when a silent batch would change nothing the skip does
//! not move on, and a skipped stretch leaves what processing it would. Both reverbs run these
//! (the space reverb's own tests are in `space_reverb`, which shares the engine).

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

/// A short decay (so a test does not wait for minutes), the modulation up, damping on.
pub(super) fn configured(h: &mut DeviceHarness) {
    for (k, v) in [
        ("algorithm", 1.0),
        ("decay", 0.3),
        ("damping", 0.4),
        ("modulation", 1.0),
        ("mix", 0.5),
        ("predelay", 20.0),
        ("early", 0.5),
    ] {
        h.set_param(k, v);
    }
}

/// Runs silent batches until the device says it is quiescent; returns how many it took.
pub(super) fn dry(h: &mut DeviceHarness, bound: usize) -> usize {
    let silence = [0.0; BATCH];
    let mut batches = 0;
    while !quiet(h) {
        feed(h, silence, silence);
        batches += 1;
        assert!(batches < bound, "it never settled");
    }
    batches
}

#[test]
fn it_is_quiescent_only_after_the_tail_ran_dry() {
    let mut h = harness(Box::new(Reverb::new()));
    configured(&mut h);
    assert!(!quiet(&h), "nothing has run");
    let silence = [0.0; BATCH];
    feed(&mut h, silence, silence);
    assert!(quiet(&h), "a fresh reverb has nothing in it");
    let mut s = 1;
    for _ in 0..40 {
        feed(&mut h, noise(&mut s), noise(&mut s));
        assert!(!quiet(&h));
    }
    let batches = dry(&mut h, 8_000);
    // The pre-delay ring alone is 16,384 samples (256 batches); the tail decays by 60 dB in 0.3 s
    // and has to fall below the flush threshold, 400 dB down.
    assert!(batches >= 256, "{batches} batches");
    h.set_param("mix", 0.4);
    assert!(!quiet(&h), "a param change is heard first");
    feed(&mut h, silence, silence);
    assert!(!quiet(&h), "the mix control has not arrived");
    dry(&mut h, 3_000);
    h.set_connected("mix_cv_in", true);
    feed(&mut h, silence, silence);
    assert!(!quiet(&h), "a connected mix CV is read every batch");
    h.set_connected("mix_cv_in", false);
    dry(&mut h, 3_000);
    // The key duck and gate are not covered: a keyed reverb is always processed.
    h.set_param("duck", 0.5);
    for _ in 0..3 {
        feed(&mut h, silence, silence);
        assert!(!quiet(&h), "a keyed reverb never rests");
    }
}

#[test]
fn a_frozen_tail_never_rests() {
    let mut h = harness(Box::new(Reverb::new()));
    configured(&mut h);
    let mut s = 9;
    for _ in 0..40 {
        feed(&mut h, noise(&mut s), noise(&mut s));
    }
    h.set_param("freeze", 1.0);
    let silence = [0.0; BATCH];
    for _ in 0..6_000 {
        feed(&mut h, silence, silence);
        assert!(!quiet(&h), "the frozen tail circulates without decay");
    }
}

/// One reverb processed through the silence, one skipped through it, with a parameter change and
/// a tempo change in the middle and the modulation running, and sound again after it.
#[test]
fn skipping_it_in_silence_leaves_what_processing_the_silence_would() {
    let make = || {
        let mut h = harness(Box::new(Reverb::new()));
        configured(&mut h);
        h
    };
    let skipped = skip_matches_process(
        &make,
        &IdleRun {
            ins: &["in_l", "in_r"],
            outs: &["out_l", "out_r"],
            batches: 20_000,
            sound: &|n| n < 30 || (12_000..12_040).contains(&n),
            edit: &|n, h| match n {
                8_000 => {
                    h.set_param("decay", 0.4);
                    true
                }
                9_000 => {
                    h.time_mut().tempo = atomik_core::Tempo::new(133.0).unwrap();
                    true
                }
                10_000 => {
                    h.set_param("algorithm", 2.0);
                    true
                }
                _ => false,
            },
        },
    );
    assert!(skipped > 8_000, "the scenario skips a lot: {skipped}");
}

/// The first notes after a long rest: the eight LFOs must stand where they would have stood, so
/// the modulated lines read the same samples. Every algorithm, the modulation at its depth.
#[test]
fn every_algorithm_resumes_with_its_modulation_in_place() {
    for algorithm in [0.0, 1.0, 2.0] {
        let make = move || {
            let mut h = harness(Box::new(Reverb::new()));
            configured(&mut h);
            h.set_param("algorithm", algorithm);
            h.set_param("modulation", 1.0);
            h.set_param("decay", 0.2);
            h
        };
        let skipped = skip_matches_process(
            &make,
            &IdleRun {
                ins: &["in_l", "in_r"],
                outs: &["out_l", "out_r"],
                batches: 6_000,
                sound: &|n| n < 20 || (5_000..5_020).contains(&n),
                edit: &|_, _| false,
            },
        );
        assert!(skipped > 2_000, "algorithm {algorithm}: {skipped}");
    }
}
