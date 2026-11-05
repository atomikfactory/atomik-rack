//! Tests of the organ: the drawbar footages and levels, the shared wheels, the keys and voices,
//! percussion, key click, scanner, overdrive, controllers, factory sounds and determinism.

use super::*;
use crate::test_support::{
    SR, assert_keys, assert_sustain_pedal, assert_volume_and_expression, harness, off, on,
    sweep_params_min_max, tone_amplitude,
};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::{dominant_frequency, max_step, peak, rms};

const OUT: [&str; 2] = ["out_l", "out_r"];
/// The output amplitude of one drawbar at full level.
const BAR_AMP: f64 = OUT_SCALE as f64;

fn seconds(s: f64) -> usize {
    (s * SR / BATCH as f64).ceil() as usize
}

/// An organ at the test sample rate with every drawbar off, no click and nothing else.
fn bare_at(sr: f64) -> DeviceHarness {
    let mut h = DeviceHarness::new(Box::new(Organ::new()), sr);
    for k in BAR_KEYS {
        h.set_param(k, 0.0);
    }
    h.set_param("key.click", 0.0);
    h
}

fn bare() -> DeviceHarness {
    bare_at(SR)
}

/// A bare organ with the drawbars at `positions`.
fn with_bars(positions: [u8; BARS]) -> DeviceHarness {
    let mut h = bare();
    for (k, p) in BAR_KEYS.iter().zip(positions) {
        h.set_param(k, f32::from(p));
    }
    h
}

/// Only the 8' drawbar, at `position`.
fn eight(position: u8) -> DeviceHarness {
    with_bars([0, 0, position, 0, 0, 0, 0, 0, 0])
}

fn hz(key: f64) -> f64 {
    440.0 * ((key - 69.0) / 12.0).exp2()
}

/// Plays `keys` from the start and returns the left output of `batches` batches.
fn play(h: &mut DeviceHarness, keys: &[u8], batches: usize) -> Vec<f32> {
    for k in keys {
        h.push_note("notes_in", on(0, *k, 1.0));
    }
    h.render(batches, &OUT).remove(0)
}

/// The amplitude of `freq` over the last second of `out` (which starts at sample `start`).
fn amp_at(out: &[f32], freq: f64, start: usize) -> f64 {
    let n = SR as usize;
    tone_amplitude(&out[out.len() - n..], freq, start + out.len() - n)
}

#[test]
fn the_descriptor_is_ordered_and_valid() {
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Instrument);
    assert_eq!(DESCRIPTOR.type_id, "atomik.organ");
    assert_keys(
        &DESCRIPTOR,
        &[
            "sound",
            "drawbar.16",
            "drawbar.5_1_3",
            "drawbar.8",
            "drawbar.4",
            "drawbar.2_2_3",
            "drawbar.2",
            "drawbar.1_3_5",
            "drawbar.1_1_3",
            "drawbar.1",
            "perc.mode",
            "perc.decay",
            "perc.level",
            "key.click",
            "scanner.type",
            "scanner.rate",
            "out.drive",
            "out.level",
        ],
        &["notes_in", "out_l", "out_r"],
    );
    let d = |k: &str| DESCRIPTOR.param_by_key(k).unwrap();
    assert_eq!(
        BAR_KEYS.map(|k| d(k).default),
        [8.0, 8.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    );
    assert!(
        BAR_KEYS
            .iter()
            .all(|k| d(k).discrete && d(k).max.to_bits() == 8.0f32.to_bits())
    );
    // A fresh device sounds like the first factory sound: its params are the defaults.
    assert!(sounds::SOUNDS[0].changes.is_empty());
}

#[test]
fn every_drawbar_sounds_at_its_equal_tempered_footage() {
    for (k, offset) in BAR_OFFSETS.iter().enumerate() {
        let mut positions = [0u8; BARS];
        positions[k] = 8;
        let mut h = with_bars(positions);
        let out = play(&mut h, &[48], seconds(1.3));
        let want = hz(48.0 + f64::from(*offset));
        let tail = &out[out.len() - 48_000..];
        let got = dominant_frequency(tail, SR);
        assert!(
            (got / want - 1.0).abs() < 0.002,
            "{}: {got} Hz, want {want}",
            BAR_NAMES[k]
        );
        // One partial and nothing else: its amplitude is the drawbar's.
        let a = amp_at(&out, want, 0);
        assert!((a / BAR_AMP - 1.0).abs() < 0.03, "{}: {a}", BAR_NAMES[k]);
        assert!(rms(tail) < (BAR_AMP * 0.72) as f32, "{}", BAR_NAMES[k]);
    }
}

#[test]
fn each_drawbar_position_is_three_db_louder_and_zero_is_off() {
    for p in 0..=8u8 {
        let mut h = eight(p);
        let out = play(&mut h, &[57], seconds(1.3));
        let a = amp_at(&out, 220.0, 0);
        let want = BAR_AMP * f64::from(bar_gain(p));
        if p == 0 {
            assert!(peak(&out) < 1e-6, "position 0 is silent");
        } else {
            assert!((a / want - 1.0).abs() < 0.01, "position {p}: {a} vs {want}");
        }
    }
    let db = |p: u8| 20.0 * f64::from(bar_gain(p)).log10();
    assert!((db(7) - db(8) + 3.0).abs() < 0.02 && (db(1) - db(8) + 21.0).abs() < 0.05);
    assert_eq!(bar_gain(8), 1.0);
}

#[test]
fn keys_that_land_on_one_pitch_share_a_wheel() {
    // The 4' drawbar of key 60 and the 8' drawbar of key 72 are the same wheel: played together
    // they add in phase, so the partial is twice one drawbar's.
    let mut h = with_bars([0, 0, 8, 8, 0, 0, 0, 0, 0]);
    let out = play(&mut h, &[60], seconds(1.3));
    let alone = amp_at(&out, hz(72.0), 0);
    assert!((alone / BAR_AMP - 1.0).abs() < 0.03, "{alone}");
    let mut h = with_bars([0, 0, 8, 8, 0, 0, 0, 0, 0]);
    let out = play(&mut h, &[60, 72], seconds(1.3));
    let shared = amp_at(&out, hz(72.0), 0);
    // Key 60's 4' plus key 72's 8': 2 * one drawbar (the wheel is common).
    assert!((shared / (2.0 * BAR_AMP) - 1.0).abs() < 0.03, "{shared}");
}

#[test]
fn a_chord_has_every_key_at_its_own_level() {
    let mut h = eight(8);
    let keys = [48u8, 52, 55, 60, 64, 67];
    let out = play(&mut h, &keys, seconds(1.3));
    for k in keys {
        let a = amp_at(&out, hz(f64::from(k)), 0);
        assert!((a / BAR_AMP - 1.0).abs() < 0.03, "key {k}: {a}");
    }
}

#[test]
fn a_high_key_loses_the_drawbars_above_the_wheel_limit() {
    // 1' of key 100 is pitch 136 (21.07 kHz): above 45 % of 44.1 kHz, below it at 48 and 96 kHz.
    let one = |sr: f64, key: u8| {
        let mut h = bare_at(sr);
        h.set_param("drawbar.1", 8.0);
        h.push_note("notes_in", on(0, key, 1.0));
        h.render(seconds(1.0), &OUT).remove(0)
    };
    assert_eq!(peak(&one(44_100.0, 100)), 0.0, "dropped at 44.1 kHz");
    assert!(peak(&one(48_000.0, 100)) > 0.1, "kept at 48 kHz");
    assert!(peak(&one(96_000.0, 100)) > 0.1, "kept at 96 kHz");
    assert_eq!(
        peak(&one(48_000.0, 110)),
        0.0,
        "34 kHz never sounds at 48 kHz"
    );
    let high = one(96_000.0, 100);
    let f = dominant_frequency(&high[high.len() - 32_768..], 96_000.0);
    assert!((f / 21_070.0 - 1.0).abs() < 0.01, "{f}");
    // The highest key does not break anything.
    let mut h = with_bars([8; BARS]);
    let out = play(&mut h, &[127, 0], seconds(0.3));
    assert!(out.iter().all(|s| s.is_finite() && s.abs() < 2.0));
}

#[test]
fn sixteen_keys_sound_and_a_seventeenth_takes_the_oldest() {
    let keys: Vec<u8> = (36..52).collect();
    let mut h = eight(8);
    // Half level, so the sixteen together stay clear of the soft limit.
    h.set_param("out.level", -6.0206);
    let out = play(&mut h, &keys, seconds(1.3));
    let each = BAR_AMP * 0.5;
    // Sixteen unrelated sines: the power adds up.
    let want = each * 8.0f64.sqrt();
    let r = f64::from(rms(&out[out.len() - 48_000..]));
    assert!((r / want - 1.0).abs() < 0.03, "{r} vs {want}");
    // The seventeenth key replaces key 36: after the swap the output is what keys 37 to 52
    // sound from the start (the wheels are the same, so the match is sample for sample).
    h.push_note("notes_in", on(0, 52, 1.0));
    let after = h.render(seconds(1.0), &OUT).remove(0);
    let mut reference = eight(8);
    reference.set_param("out.level", -6.0206);
    let others: Vec<u8> = (37..=52).collect();
    play(&mut reference, &others, seconds(1.3));
    let want = reference.render(seconds(1.0), &OUT).remove(0);
    let n = after.len() - 4_800;
    let worst = after[4_800..]
        .iter()
        .zip(&want[want.len() - n..])
        .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(worst < 1e-4, "{worst}");
}

#[test]
fn a_key_struck_again_while_sounding_keeps_one_voice() {
    let mut h = eight(8);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.render(seconds(0.5), &OUT);
    let out = play(&mut h, &[60, 60], seconds(1.3));
    let a = amp_at(&out, hz(60.0), 0);
    assert!((a / BAR_AMP - 1.0).abs() < 0.03, "{a}");
}

#[test]
fn released_keys_fall_silent_exactly() {
    let mut h = with_bars([8; BARS]);
    play(&mut h, &[40, 52, 64], seconds(0.2));
    for k in [40u8, 52, 64] {
        h.push_note("notes_in", off(0, k));
    }
    let out = h.render(seconds(0.1), &OUT).remove(0);
    assert!(
        peak(&out[out.len() - 480..]) < 1e-3,
        "released within 100 ms"
    );
    let tail = h.render(seconds(0.5), &OUT).remove(0);
    assert!(tail.iter().all(|s| *s == 0.0), "idle voices write zeros");
}

#[test]
fn all_off_releases_every_key() {
    let mut h = eight(8);
    play(&mut h, &[40, 52, 64], seconds(0.2));
    h.push_note(
        "notes_in",
        NoteEvent {
            offset: 0,
            kind: NoteKind::AllOff,
        },
    );
    h.render(seconds(0.2), &OUT);
    let tail = h.render(seconds(0.2), &OUT).remove(0);
    assert!(tail.iter().all(|s| *s == 0.0));
}

#[test]
fn a_note_starts_at_its_frame_without_a_step() {
    let mut h = eight(8);
    h.push_note("notes_in", on(20, 45, 1.0));
    let out = h.render(seconds(0.5), &OUT).remove(0);
    assert!(out[..20].iter().all(|s| *s == 0.0));
    assert!(out[20..400].iter().any(|s| s.abs() > 1e-4));
    // The key envelope: no click at the press or the release even at a crest of the tone.
    assert!(max_step(&out) < 0.03, "{}", max_step(&out));
    h.push_note("notes_in", off(3, 45));
    let out = h.render(seconds(0.2), &OUT).remove(0);
    assert!(max_step(&out) < 0.03, "{}", max_step(&out));
}

#[test]
fn the_wheels_are_phase_locked_across_the_run() {
    // Two seconds of a held chord keep their amplitudes: the phasors do not drift or decay.
    let mut h = eight(8);
    let out = play(&mut h, &[48, 60, 72], seconds(4.0));
    let late = &out[out.len() - 48_000..];
    let early = &out[12_000..12_000 + 48_000];
    let a = f64::from(rms(early));
    let b = f64::from(rms(late));
    assert!((a / b - 1.0).abs() < 0.01, "{a} vs {b}");
    assert!((a / (BAR_AMP * 1.5f64.sqrt()) - 1.0).abs() < 0.05, "{a}");
}

// ----------------------------------------------------------------------------------- percussion

/// The amplitude of `freq` in `[from, from + len)` samples of `out`.
fn window(out: &[f32], freq: f64, from: usize, len: usize) -> f64 {
    tone_amplitude(&out[from..from + len], freq, from)
}

/// An organ with every drawbar off, so only the percussion sounds.
fn perc(mode: f32, decay: f32, level: f32) -> DeviceHarness {
    let mut h = bare();
    h.set_param("perc.mode", mode);
    h.set_param("perc.decay", decay);
    h.set_param("perc.level", level);
    h
}

#[test]
fn percussion_adds_a_decaying_second_or_third_harmonic() {
    // The percussion harmonics are those of the key: for key 48 the 4' (key 60) and the 2 2/3'
    // (key 67).
    for (mode, tone) in [(1.0, hz(60.0)), (2.0, hz(67.0))] {
        let mut h = perc(mode, 0.0, 1.0);
        let out = play(&mut h, &[48], seconds(1.0));
        let early = window(&out, tone, 0, 960);
        let late = window(&out, tone, 9_600, 4_800);
        assert!(early > BAR_AMP * 0.6, "mode {mode}: {early}");
        assert!(late < BAR_AMP * 0.01, "fast decay is gone by 0.2 s: {late}");
    }
    // Off adds nothing.
    let mut h = perc(0.0, 0.0, 1.0);
    let out = play(&mut h, &[48], seconds(0.5));
    assert!(window(&out, hz(60.0), 0, 960) < BAR_AMP * 0.02);
}

#[test]
fn percussion_decays_by_sixty_db_in_its_time() {
    let level_at = |decay: f32, seconds_in: f64| {
        let mut h = perc(1.0, decay, 1.0);
        let out = play(&mut h, &[48], seconds(1.2));
        let from = (seconds_in * SR) as usize - 600;
        window(&out, hz(60.0), from, 1_200) / BAR_AMP
    };
    // Fast: 60 dB in 0.2 s, so 0.1 s in is -30 dB (0.0316); slow: 0.9 s, so 0.45 s is -30 dB.
    let fast = level_at(0.0, 0.1);
    assert!((fast / 0.0316 - 1.0).abs() < 0.25, "{fast}");
    let slow = level_at(1.0, 0.45);
    assert!((slow / 0.0316 - 1.0).abs() < 0.25, "{slow}");
    assert!(level_at(1.0, 0.1) > level_at(0.0, 0.1) * 10.0);
}

#[test]
fn soft_percussion_is_six_db_lower() {
    let early = |level: f32| {
        let mut h = perc(1.0, 1.0, level);
        let out = play(&mut h, &[48], seconds(0.2));
        window(&out, hz(60.0), 480, 960)
    };
    let ratio = early(0.0) / early(1.0);
    assert!((ratio - 0.5).abs() < 0.02, "{ratio}");
}

#[test]
fn percussion_silences_the_one_foot_drawbar() {
    let one_foot = |mode: f32| {
        let mut h = with_bars([0, 0, 8, 0, 0, 0, 0, 0, 8]);
        h.set_param("perc.mode", mode);
        let out = play(&mut h, &[48], seconds(1.3));
        amp_at(&out, hz(84.0), 0)
    };
    assert!((one_foot(0.0) / BAR_AMP - 1.0).abs() < 0.03);
    assert!(one_foot(1.0) < BAR_AMP * 0.01);
}

#[test]
fn percussion_restarts_only_when_no_key_is_held() {
    // Keys 48 then 55 (whose 2nd harmonic is key 67) at 0.5 s, fast decay: when 48 is still held
    // the percussion does not restart; when it was released first, it does.
    let run = |release_first: bool| {
        let mut h = perc(1.0, 0.0, 1.0);
        h.push_note("notes_in", on(0, 48, 1.0));
        h.render(seconds(0.5), &OUT);
        if release_first {
            h.push_note("notes_in", off(0, 48));
            h.render(seconds(0.2), &OUT);
        }
        h.push_note("notes_in", on(0, 55, 1.0));
        let out = h.render(seconds(0.2), &OUT).remove(0);
        window(&out, hz(67.0), 0, 960)
    };
    assert!(run(false) < BAR_AMP * 0.01, "held: no new strike");
    assert!(run(true) > BAR_AMP * 0.5, "released: struck again");
}

// ------------------------------------------------------------------------------------ key click

fn clicked(click: f32, seed: u64) -> Vec<f32> {
    let mut h = DeviceHarness::with_options(Box::new(Organ::new()), SR, 120.0, seed);
    for k in BAR_KEYS {
        h.set_param(k, 0.0);
    }
    h.set_param("drawbar.8", 8.0);
    h.set_param("key.click", click);
    h.push_note("notes_in", on(0, 57, 1.0));
    let mut out = h.render(seconds(0.3), &OUT).remove(0);
    h.push_note("notes_in", off(0, 57));
    out.extend(h.render(seconds(0.3), &OUT).remove(0));
    out
}

#[test]
fn the_key_click_is_a_short_burst_at_the_press_and_a_smaller_one_at_the_release() {
    let none = clicked(0.0, 1);
    let full = clicked(1.0, 1);
    let diff: Vec<f32> = full.iter().zip(&none).map(|(a, b)| a - b).collect();
    let at = |from: usize, len: usize| f64::from(rms(&diff[from..from + len]));
    let press = at(0, 240);
    assert!(press > 0.01, "{press}");
    assert!(at(2_400, 2_400) < press * 1e-3, "it has died away by 50 ms");
    let release = at(14_400, 240);
    assert!(
        release > press * 0.2 && release < press * 0.7,
        "{release} vs {press}"
    );
    // It scales with the knob.
    let half = clicked(0.5, 1);
    let diff_half: Vec<f32> = half.iter().zip(&none).map(|(a, b)| a - b).collect();
    let r = f64::from(rms(&diff_half[..240])) / press;
    assert!((r - 0.5).abs() < 0.02, "{r}");
    // The noise follows the device seed.
    assert_ne!(clicked(1.0, 1), clicked(1.0, 2));
    assert_eq!(clicked(1.0, 2), clicked(1.0, 2));
}

// -------------------------------------------------------------------------------------- scanner

fn scanned(kind: f32, rate: f32) -> (Vec<f32>, Vec<f32>) {
    let mut h = eight(8);
    h.set_param("scanner.type", kind);
    h.set_param("scanner.rate", rate);
    h.push_note("notes_in", on(0, 69, 1.0));
    let mut o = h.render(seconds(1.4), &OUT);
    (o.remove(0), o.remove(0))
}

#[test]
fn the_vibrato_makes_sidebands_that_grow_with_its_depth() {
    let side = |kind: f32| {
        let (l, _) = scanned(kind, 8.0);
        let from = l.len() - 48_000;
        let up = tone_amplitude(&l[from..], 448.0, from);
        let down = tone_amplitude(&l[from..], 432.0, from);
        (up, down)
    };
    let (up0, down0) = side(0.0);
    assert!(up0 < 1e-4 && down0 < 1e-4, "Off is a pure tone");
    let (u1, d1) = side(1.0);
    let (u2, d2) = side(2.0);
    let (u3, d3) = side(3.0);
    // Phase modulation by 2 pi f d: index 0.33, 0.66 and 1.16; J1 of those times the amplitude.
    assert!(
        (u1 / 0.0244 - 1.0).abs() < 0.15 && (d1 / 0.0244 - 1.0).abs() < 0.15,
        "{u1} {d1}"
    );
    assert!(u2 > u1 * 1.7 && u3 > u2 * 1.3, "{u1} {u2} {u3}");
    assert!(
        (u3 / 0.073 - 1.0).abs() < 0.15 && (d3 / 0.073 - 1.0).abs() < 0.15,
        "{u3} {d3}"
    );
    assert!(d2 > d1);
}

#[test]
fn the_scan_rate_sets_where_the_sidebands_are() {
    let (l, _) = scanned(3.0, 5.0);
    let from = l.len() - 48_000;
    assert!(tone_amplitude(&l[from..], 445.0, from) > 0.05);
    assert!(tone_amplitude(&l[from..], 448.0, from) < 0.01);
}

#[test]
fn vibrato_is_mono_and_chorus_is_wide() {
    for kind in 0..=3 {
        let (l, r) = scanned(kind as f32, 6.8);
        assert_eq!(l, r, "type {kind}");
    }
    for kind in 4..=6 {
        let (l, r) = scanned(kind as f32, 8.0);
        assert_ne!(l, r, "type {kind}");
        // Half of the signal is the direct tone, so the sidebands are about half of V's.
        let from = l.len() - 48_000;
        let side = tone_amplitude(&l[from..], 448.0, from);
        assert!(side > 0.005 && side < 0.045, "type {kind}: {side}");
    }
}

#[test]
fn changing_the_scanner_during_a_note_glides_without_a_step() {
    let mut h = eight(8);
    h.push_note("notes_in", on(0, 69, 1.0));
    h.render(seconds(0.2), &OUT);
    let mut out = Vec::new();
    for kind in [3.0, 0.0, 6.0, 1.0] {
        h.set_param("scanner.type", kind);
        out.extend(h.render(seconds(0.2), &OUT).remove(0));
    }
    assert!(max_step(&out) < 0.05, "{}", max_step(&out));
    assert!(out.iter().all(|s| s.is_finite()));
}

// -------------------------------------------------------------------------------- drive and level

#[test]
fn overdrive_adds_odd_harmonics_and_zero_is_off() {
    let third = |drive: f32| {
        let mut h = eight(8);
        h.set_param("out.drive", drive);
        let out = play(&mut h, &[57], seconds(1.3));
        (
            amp_at(&out, 660.0, 0),
            amp_at(&out, 440.0, 0),
            amp_at(&out, 220.0, 0),
        )
    };
    let (clean3, _, clean1) = third(0.0);
    assert!(clean3 < 1e-4, "no harmonics without drive: {clean3}");
    let (drv3, drv2, drv1) = third(1.0);
    assert!(drv3 > 0.005, "{drv3}");
    assert!(
        drv2 < 1e-4,
        "an odd-symmetric clipper has no even harmonics: {drv2}"
    );
    assert!(drv1 > clean1 * 0.5);
    // Zero is bit-exact: the same bits as a device whose drive was never touched.
    let mut a = eight(8);
    let mut b = eight(8);
    b.set_param("out.drive", 0.0);
    assert_eq!(play(&mut a, &[57], 50), play(&mut b, &[57], 50));
}

#[test]
fn the_level_is_a_gain_in_db_and_the_output_stays_below_two() {
    let amp = |db: f32| {
        let mut h = eight(8);
        h.set_param("out.level", db);
        let out = play(&mut h, &[57], seconds(1.3));
        amp_at(&out, 220.0, 0)
    };
    let a0 = amp(0.0);
    assert!((amp(-6.0206) / a0 - 0.5).abs() < 0.005);
    assert!(amp(-24.0) < a0 * 0.07);
    // The loudest setting of everything on many keys.
    let mut h = with_bars([8; BARS]);
    h.set_param("out.level", 6.0);
    h.set_param("out.drive", 1.0);
    let keys: Vec<u8> = (30..46).collect();
    let out = play(&mut h, &keys, seconds(1.0));
    assert!(peak(&out) < 2.0 && peak(&out) > 0.5, "{}", peak(&out));
}

// ------------------------------------------------------------------------- tune and controllers

#[test]
fn the_master_tune_moves_every_wheel() {
    let mut h = eight(8);
    h.time_mut().tune_cents = 100.0;
    let out = play(&mut h, &[57], seconds(1.3));
    let f = dominant_frequency(&out[out.len() - 48_000..], SR);
    assert!(
        (f / (220.0 * (1.0f64 / 12.0).exp2()) - 1.0).abs() < 0.001,
        "{f}"
    );
    h.time_mut().tune_cents = 0.0;
    let out = h.render(seconds(1.3), &OUT).remove(0);
    let f = dominant_frequency(&out[out.len() - 48_000..], SR);
    assert!((f / 220.0 - 1.0).abs() < 0.001, "{f}");
    assert!(out.iter().all(|s| s.is_finite()));
}

#[test]
fn the_sustain_pedal_holds_keys_and_release_follows_the_pedal() {
    assert_sustain_pedal(|| eight(8), &OUT, 57);
}

#[test]
fn volume_and_expression_follow_a_square_law() {
    assert_volume_and_expression(|| eight(8), &OUT, 57);
}

#[test]
fn the_bend_wheel_and_pressure_are_ignored() {
    let base = play(&mut eight(8), &[57], 100);
    let mut h = eight(8);
    for kind in [
        NoteKind::PitchBend { value: 1.0 },
        NoteKind::ChannelPressure { value: 1.0 },
        NoteKind::Cc {
            controller: 1,
            value: 1.0,
        },
    ] {
        h.push_note("notes_in", NoteEvent { offset: 0, kind });
    }
    assert_eq!(play(&mut h, &[57], 100), base);
}

// ----------------------------------------------------------------------- determinism and bounds

fn script(seed: u64) -> Vec<f32> {
    let mut h = DeviceHarness::with_options(Box::new(Organ::new()), SR, 120.0, seed);
    h.set_param("scanner.type", 6.0);
    h.set_param("perc.mode", 2.0);
    h.set_param("out.drive", 0.4);
    for i in 0..200u32 {
        let key = 36 + (i * 7 % 40) as u8;
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
}

#[test]
fn it_is_allocation_free_and_deterministic() {
    // `process` runs under the allocation guard in the harness.
    assert_eq!(script(2), script(2));
    assert_ne!(script(2), script(3), "the wheel phases follow the seed");
}

#[test]
fn a_reset_returns_to_the_first_render() {
    let run = |h: &mut DeviceHarness| {
        h.set_param("scanner.type", 5.0);
        h.set_param("perc.mode", 1.0);
        let mut out = play(h, &[40, 47, 55], 80);
        h.push_note("notes_in", off(0, 47));
        out.extend(h.render(40, &OUT).remove(0));
        out
    };
    let mut h = eight(8);
    let first = run(&mut h);
    h.device_mut().reset();
    assert_eq!(run(&mut h), first);
}

#[test]
fn the_sound_param_alone_changes_nothing() {
    let base = play(&mut eight(8), &[57], 100);
    let mut h = eight(8);
    h.set_param("sound", 3.0);
    assert_eq!(play(&mut h, &[57], 100), base);
}

#[test]
fn every_param_min_max_stays_finite() {
    let mut h = harness(Box::new(Organ::new()));
    for k in [24u8, 36, 60, 96, 127] {
        h.push_note("notes_in", on(0, k, 1.0));
    }
    sweep_params_min_max(&mut h, &DESCRIPTOR);
    let mut h = harness(Box::new(Organ::new()));
    for p in ["perc.mode", "scanner.type", "out.drive"] {
        h.set_param(p, DESCRIPTOR.param_by_key(p).unwrap().max);
    }
    let out = play(&mut h, &[30, 50, 70, 90], 200);
    assert!(out.iter().all(|s| s.is_finite() && s.abs() < 2.0));
}

#[test]
fn bad_values_are_clamped() {
    let mut h = eight(8);
    for p in DESCRIPTOR.params {
        h.device_mut().set_param(p.id, f32::NAN);
        h.device_mut().set_param(p.id, f32::INFINITY);
        h.device_mut().set_param(p.id, -1.0e9);
    }
    let out = play(&mut h, &[57, 200], 40);
    assert!(out.iter().all(|s| s.is_finite() && s.abs() < 2.0));
}

// ------------------------------------------------------------------------------------- sounds

#[test]
fn the_factory_sounds_are_complete_and_in_range() {
    assert_eq!(sounds::SOUNDS.len(), sounds::SOUND_NAMES.len());
    for (i, s) in sounds::SOUNDS.iter().enumerate() {
        assert_eq!(s.name, sounds::SOUND_NAMES[i]);
        for (key, v) in s.changes {
            let p = DESCRIPTOR
                .param_by_key(key)
                .unwrap_or_else(|| panic!("{}: no param {key}", s.name));
            assert!(*v >= p.min && *v <= p.max, "{}: {key} = {v}", s.name);
            assert!(!p.discrete || v.fract() == 0.0, "{}: {key}", s.name);
            assert_ne!(*key, "sound");
        }
        let all = sounds::values(i);
        assert_eq!(all.len(), PARAMS.len() - 1);
    }
    assert_eq!(sounds::values(99).len(), PARAMS.len() - 1);
    assert_eq!(
        DESCRIPTOR.param_by_key("sound").unwrap().unit,
        Unit::Choice(&sounds::SOUND_NAMES)
    );
}

#[test]
fn every_factory_sound_sounds_and_they_differ() {
    let mut renders = Vec::new();
    for i in 0..sounds::SOUNDS.len() {
        let mut h = harness(Box::new(Organ::new()));
        for (key, v) in sounds::values(i) {
            h.set_param(key, v);
        }
        let out = play(&mut h, &[48, 55, 60, 64], seconds(1.0));
        assert!(
            out.iter().all(|s| s.is_finite() && s.abs() < 2.0),
            "{}",
            sounds::SOUND_NAMES[i]
        );
        assert!(
            rms(&out[out.len() / 2..]) > 0.05,
            "{}",
            sounds::SOUND_NAMES[i]
        );
        renders.push(out.iter().map(|s| s.to_bits()).collect::<Vec<_>>());
    }
    for a in 0..renders.len() {
        for b in a + 1..renders.len() {
            assert_ne!(renders[a], renders[b], "sounds {a} and {b}");
        }
    }
}
