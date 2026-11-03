//! The synthesised pad modules: sound, pitch, determinism, choke, stealing, no allocation and
//! bounded output.

#![allow(clippy::float_cmp)]
use super::*;
use atomik_core::BATCH;
use atomik_testkit::signal::{dominant_frequency, peak, rms};

const MODULE_NAMES: [&str; 6] = ["kick", "snare", "hat", "tom", "tone", "noise"];

/// A drum machine whose pad 1 plays module `index` (1 = kick .. 6 = noise), no sample map.
fn synth(index: usize) -> DeviceHarness {
    let (mut h, _) = drum();
    h.set_param("pad1.module", index as f32);
    h.set_param("pad1.mode", 1.0);
    h
}

/// One full-velocity hit of pad 1 rendered for `samples` frames (a multiple of 64).
fn pad1(h: &mut DeviceHarness, samples: usize) -> (Vec<f32>, Vec<f32>) {
    hit(h, 0, 1, 1.0);
    render(h, samples / BATCH)
}

#[test]
fn every_module_sounds_without_a_sample_map_and_stays_in_range() {
    for (i, name) in MODULE_NAMES.iter().enumerate() {
        let mut h = synth(i + 1);
        h.set_param("pad1.decay", 300.0);
        let (l, r) = pad1(&mut h, 24_000);
        let p = peak(&l).max(peak(&r));
        assert!(
            (0.05..=1.5).contains(&p),
            "{name} peaks at {p}, expected a usable level"
        );
        assert!(l.iter().chain(&r).all(|s| s.is_finite()), "{name}");
        // The voice ends by itself: the last batches are silent.
        let tail = &l[l.len() - 256..];
        assert!(peak(tail) < 1.0e-3, "{name} did not decay");
    }
}

#[test]
fn a_sample_pad_is_untouched_by_the_module_params() {
    let kit = || flat_kit(&[1], 0.5, 4_000);
    let (mut a, _) = with_map(&kit());
    let base = pad1(&mut a, 20);
    let (mut b, _) = with_map(&kit());
    for k in ["body", "snap", "sweep"] {
        b.set_param(&format!("pad1.{k}"), 0.9);
    }
    // Choosing a module and going back leaves the sample exactly as it was.
    b.set_param("pad1.module", 3.0);
    b.set_param("pad1.module", 0.0);
    let moved = pad1(&mut b, 20);
    assert_eq!(bits(&base.0), bits(&moved.0));
    assert_eq!(bits(&base.1), bits(&moved.1));
}

#[test]
fn a_module_pad_ignores_its_zones_and_a_sample_pad_beside_it_still_plays() {
    let (mut h, _) = with_map(&flat_kit(&[1, 2], 0.5, 4_000));
    h.set_param("pad1.module", 5.0);
    hit(&mut h, 0, 2, 1.0);
    let (l, _) = render(&mut h, 4);
    // Pad 2 is a plain sample: a flat 0.5 at the centre of the pan.
    assert!((l[100] - 0.5).abs() < 1e-4, "{}", l[100]);
    let (mut h, _) = with_map(&flat_kit(&[1, 2], 0.5, 4_000));
    h.set_param("pad1.module", 5.0);
    let (l, _) = pad1(&mut h, 4_096);
    assert!(
        (l[2_000] - 0.5).abs() > 1e-3,
        "the tone module played the flat sample"
    );
}

#[test]
fn tone_follows_tune_and_the_master_tune() {
    let freq = |tune: f32| {
        let mut h = synth(5);
        h.set_param("pad1.sweep", 0.0);
        h.set_param("pad1.snap", 0.0);
        h.set_param("pad1.body", 0.0);
        h.set_param("pad1.decay", 2_000.0);
        h.set_param("pad1.tune", tune);
        let (l, _) = pad1(&mut h, 24_000);
        dominant_frequency(&l[2_000..2_000 + 16_384], SR)
    };
    let (c4, c5) = (freq(0.0), freq(12.0));
    assert!((c4 - 261.6).abs() < 5.0, "{c4}");
    assert!((c5 / c4 - 2.0).abs() < 0.03, "{c4} {c5}");
}

#[test]
fn the_kick_sweeps_down_and_the_sweep_knob_sets_how_far() {
    let early = |sweep: f32| {
        let mut h = synth(1);
        h.set_param("pad1.sweep", sweep);
        h.set_param("pad1.snap", 0.0);
        h.set_param("pad1.body", 0.0);
        h.set_param("pad1.decay", 1_500.0);
        let (l, _) = pad1(&mut h, 14_400);
        // Zero crossings in the first 10 ms against the ones from 150 ms on.
        let crossings = |x: &[f32]| x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        (crossings(&l[..480]), crossings(&l[7_200..7_200 + 4_800]))
    };
    let (flat_early, flat_late) = early(0.0);
    let (deep_early, deep_late) = early(1.0);
    assert!(deep_early > flat_early + 1, "{deep_early} {flat_early}");
    // The tail settles on the same base pitch.
    assert!(
        deep_late.abs_diff(flat_late) <= 1,
        "{deep_late} {flat_late}"
    );
}

#[test]
fn snap_and_body_move_the_balance_of_the_snare_and_the_hat() {
    for index in [2, 3] {
        let level = |body: f32, snap: f32| {
            let mut h = synth(index);
            h.set_param("pad1.body", body);
            h.set_param("pad1.snap", snap);
            h.set_param("pad1.decay", 300.0);
            rms(&pad1(&mut h, 4_800).0)
        };
        let (shell, noise, both) = (level(1.0, 0.0), level(0.0, 1.0), level(1.0, 1.0));
        assert!(shell > 0.005 && noise > 0.005, "{index}: {shell} {noise}");
        assert_eq!(level(0.0, 0.0), 0.0);
        assert!(both > shell.max(noise) * 0.9, "{index}: {both}");
    }
}

#[test]
fn modules_are_deterministic_and_the_same_hit_sounds_the_same_twice() {
    for index in 1..=6 {
        let run = || {
            let mut h = synth(index);
            h.set_param("pad1.decay", 400.0);
            pad1(&mut h, 24_000)
        };
        let (a, b) = (run(), run());
        assert_eq!(bits(&a.0), bits(&b.0), "{index}");
        assert_eq!(bits(&a.1), bits(&b.1), "{index}");
    }
}

#[test]
fn a_module_pad_chokes_and_is_choked_like_a_sample_pad() {
    let (mut h, probe) = drum();
    for (pad, m) in [(1, 3.0), (2, 3.0)] {
        h.set_param(&format!("pad{pad}.module"), m);
        h.set_param(&format!("pad{pad}.choke"), 1.0);
        h.set_param(&format!("pad{pad}.decay"), 2_000.0);
    }
    hit(&mut h, 0, 1, 1.0);
    render(&mut h, 4);
    assert_eq!(probe.active(), 1);
    hit(&mut h, 0, 2, 1.0);
    render(&mut h, 4);
    // Pad 1 faded out within the 2 ms choke, pad 2 is the only voice left.
    assert_eq!(probe.keys(), vec![pad_key(1)]);
}

#[test]
fn a_gate_mode_module_holds_until_the_note_off() {
    let mut h = synth(5);
    h.set_param("pad1.mode", 0.0);
    h.set_param("pad1.decay", 100.0);
    h.set_param("pad1.snap", 0.0);
    hit(&mut h, 0, 1, 1.0);
    let (held, _) = render(&mut h, 80);
    assert!(
        rms(&held[held.len() - 512..]) > 0.1,
        "the tone fell while held"
    );
    h.push_note("notes_in", off(0, key_of(1)));
    let (rest, _) = render(&mut h, 80);
    assert!(peak(&rest[rest.len() - 256..]) < 1.0e-3);
}

#[test]
fn a_module_hit_steals_the_oldest_voice_when_all_are_busy() {
    let (mut h, probe) = drum();
    for p in 1..=PAD_COUNT {
        h.set_param(&format!("pad{p}.module"), (1 + p % 6) as f32);
        h.set_param(&format!("pad{p}.decay"), 5_000.0);
        h.set_param(&format!("pad{p}.mode"), 0.0);
    }
    // 40 hits on 16 pads at different offsets, over the 32 voices.
    for n in 0..40u16 {
        hit(&mut h, n * 3, 1 + usize::from(n) % PAD_COUNT, 1.0);
    }
    let (l, r) = render(&mut h, 4);
    assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 16.0));
    assert!(probe.active() <= VOICES);
}

#[test]
fn module_voices_reach_the_pads_own_outputs_and_effects() {
    let mut h = synth(1);
    connect_pad(&mut h, 1);
    hit(&mut h, 0, 1, 1.0);
    h.process();
    let [pl, _] = pad_out_keys(1);
    assert!(peak(h.output(&pl)) > 0.01);
    assert!(
        peak(h.output("out_l")) == 0.0,
        "a routed pad left the main mix"
    );
}

#[test]
fn modules_do_not_allocate_and_stay_finite_for_any_values() {
    let (mut h, _) = drum();
    for p in 1..=PAD_COUNT {
        h.set_param(&format!("pad{p}.module"), (1 + (p - 1) % 6) as f32);
        h.set_param(&format!("pad{p}.drive"), 0.5);
    }
    for p in 1..=PAD_COUNT {
        hit(&mut h, p as u16, p, 1.0);
    }
    h.process();
    assert_no_alloc(|| {
        for _ in 0..30 {
            h.process();
        }
    });
    for v in [f32::NAN, -5.0, 1.0e9, 0.0, 1.0] {
        for k in ["module", "body", "snap", "sweep", "tune", "decay"] {
            h.set_param("pad1.module", 2.0);
            h.set_param(&format!("pad1.{k}"), v);
            hit(&mut h, 0, 1, 1.0);
            assert_no_alloc(|| {
                h.process();
                h.process();
            });
            assert!(h.output("out_l").iter().all(|s| s.is_finite()), "{k} {v}");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn any_module_settings_and_hits_keep_the_output_bounded(
        module in 1u8..=6,
        body in 0.0f32..=1.0,
        snap in 0.0f32..=1.0,
        sweep in 0.0f32..=1.0,
        tune in -24.0f32..=24.0,
        decay in 5.0f32..=10_000.0,
        mut hits in prop::collection::vec((0u16..64, 1usize..=16, 0.01f32..=1.0), 1..24),
    ) {
        hits.sort_by_key(|h| h.0);
        let (mut h, _) = drum();
        for p in 1..=PAD_COUNT {
            h.set_param(&format!("pad{p}.module"), f32::from(module));
            h.set_param(&format!("pad{p}.body"), body);
            h.set_param(&format!("pad{p}.snap"), snap);
            h.set_param(&format!("pad{p}.sweep"), sweep);
            h.set_param(&format!("pad{p}.tune"), tune);
            h.set_param(&format!("pad{p}.decay"), decay);
        }
        for (offset, pad, velocity) in hits {
            hit(&mut h, offset, pad, velocity);
        }
        let (l, r) = render(&mut h, 40);
        for s in l.iter().chain(&r) {
            prop_assert!(s.is_finite() && s.abs() <= 16.0, "{s}");
        }
    }
}
