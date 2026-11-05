//! Tests of the note extraction on synthetic signals with known notes and hits.

use super::*;
use crate::Rng;

const SR: u32 = 48_000;

fn frames(seconds: f64) -> usize {
    (seconds * f64::from(SR)).round() as usize
}

/// A sine note of `hz` from `from` to `to` seconds (5 ms linear fades), added to `x`.
fn note(x: &mut [f32], hz: f64, from: f64, to: f64, amp: f32) {
    let (a, b) = (frames(from), frames(to).min(x.len()));
    let fade = frames(0.005) as f64;
    for (i, v) in x[a..b].iter_mut().enumerate() {
        let t = i as f64 / f64::from(SR);
        let env = (i as f64 / fade).min(((b - a - i) as f64) / fade).min(1.0);
        *v += amp * (env * (std::f64::consts::TAU * hz * t).sin()) as f32;
    }
}

fn hz(key: u8) -> f64 {
    440.0 * ((f64::from(key) - 69.0) / 12.0).exp2()
}

/// Notes of a melody: `(key, from, to)` in seconds.
const MELODY: [(u8, f64, f64); 5] = [
    (60, 0.10, 0.50),
    (64, 0.60, 1.00),
    (67, 1.00, 1.40),
    (72, 1.60, 2.00),
    (48, 2.20, 2.70),
];

fn melody() -> Vec<f32> {
    let mut x = vec![0.0f32; frames(3.0)];
    for (key, from, to) in MELODY {
        note(&mut x, hz(key), from, to, 0.5);
    }
    x
}

fn secs(frames: u64) -> f64 {
    frames as f64 / f64::from(SR)
}

#[test]
fn a_sine_melody_is_recovered_with_its_keys_and_timings() {
    let x = melody();
    let notes = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    let got: Vec<(u8, f64, f64)> = notes
        .iter()
        .map(|n| (n.key, secs(n.start), secs(n.start + n.length)))
        .collect();
    assert_eq!(notes.len(), MELODY.len(), "{got:?}");
    for (n, (key, from, to)) in got.iter().zip(MELODY) {
        assert_eq!(n.0, key, "{got:?}");
        assert!(
            (n.1 - from).abs() < 0.03,
            "start {} for {from}: {got:?}",
            n.1
        );
        assert!((n.2 - to).abs() < 0.04, "end {} for {to}: {got:?}", n.2);
    }
    for n in &notes {
        assert!(n.velocity > 0.0 && n.velocity <= 1.0);
    }
}

#[test]
fn notes_do_not_overlap_and_are_sorted() {
    let x = melody();
    let notes = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    for w in notes.windows(2) {
        assert!(w[0].start + w[0].length <= w[1].start, "{notes:?}");
    }
}

#[test]
fn a_vibrato_does_not_split_a_note() {
    // A3 with 5.5 Hz vibrato of +-40 cents for 1.2 s.
    let n = frames(1.6);
    let mut x = vec![0.0f32; n];
    let mut phase = 0.0f64;
    let (a, b) = (frames(0.2), frames(1.4));
    for (i, v) in x.iter_mut().enumerate().take(b).skip(a) {
        let t = i as f64 / f64::from(SR);
        let cents = 40.0 * (std::f64::consts::TAU * 5.5 * t).sin();
        let f = hz(57) * (cents / 1200.0).exp2();
        phase += std::f64::consts::TAU * f / f64::from(SR);
        *v = 0.5 * phase.sin() as f32;
    }
    let notes = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].key, 57);
    assert!((secs(notes[0].start) - 0.2).abs() < 0.03);
    assert!((secs(notes[0].start + notes[0].length) - 1.4).abs() < 0.05);
}

#[test]
fn a_glide_inside_the_tolerance_does_not_split_a_note() {
    // E4 sliding up by 0.6 semitone over 150 ms, then holding.
    let n = frames(1.2);
    let mut x = vec![0.0f32; n];
    let mut phase = 0.0f64;
    let (a, b) = (frames(0.1), frames(1.0));
    for (i, v) in x.iter_mut().enumerate().take(b).skip(a) {
        let t = (i - a) as f64 / f64::from(SR);
        let semis = 0.6 * (t / 0.15).min(1.0);
        phase += std::f64::consts::TAU * hz(64) * (semis / 12.0).exp2() / f64::from(SR);
        *v = 0.5 * phase.sin() as f32;
    }
    let notes = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    assert_eq!(notes.len(), 1, "{notes:?}");
}

#[test]
fn a_re_attack_on_the_same_pitch_makes_two_notes() {
    // Two A3s with a 30 ms gap, and two D4s with a 20 ms gap (shorter than the 25 ms that a
    // note bridges, so only the dip in level tells them apart).
    let mut x = vec![0.0f32; frames(2.4)];
    note(&mut x, hz(57), 0.1, 0.5, 0.5);
    note(&mut x, hz(57), 0.53, 0.9, 0.5);
    note(&mut x, hz(62), 1.2, 1.6, 0.5);
    note(&mut x, hz(62), 1.62, 2.0, 0.5);
    let notes = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    let keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
    assert_eq!(keys, vec![57, 57, 62, 62], "{notes:?}");
    assert!((secs(notes[1].start) - 0.53).abs() < 0.03, "{notes:?}");
    assert!((secs(notes[3].start) - 1.62).abs() < 0.03, "{notes:?}");
}

#[test]
fn a_harmonic_tone_gets_its_fundamental() {
    let mut x = vec![0.0f32; frames(1.0)];
    for (h, amp) in [(1.0, 0.25), (2.0, 0.3), (3.0, 0.2), (4.0, 0.1)] {
        note(&mut x, hz(52) * h, 0.1, 0.8, amp);
    }
    let notes = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].key, 52);
}

#[test]
fn a_decaying_note_ends_where_its_level_has_fallen() {
    // A plucked A3: exponential decay with a 60 ms time constant, then nothing more.
    let mut x = vec![0.0f32; frames(2.0)];
    let f = hz(57);
    for (i, v) in x.iter_mut().enumerate().skip(frames(0.1)) {
        let t = (i - frames(0.1)) as f64 / f64::from(SR);
        *v = (0.8 * (-t / 0.06).exp() * (std::f64::consts::TAU * f * t).sin()) as f32;
    }
    let notes = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    assert_eq!(notes.len(), 1, "{notes:?}");
    // 30 dB down is 3.45 time constants: about 0.2 s.
    let len = secs(notes[0].length);
    assert!((0.1..0.45).contains(&len), "{notes:?}");
}

#[test]
fn velocity_follows_the_level() {
    let mut x = vec![0.0f32; frames(2.0)];
    note(&mut x, hz(60), 0.1, 0.6, 0.8);
    note(&mut x, hz(60), 0.9, 1.4, 0.2);
    let notes = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!((notes[0].velocity - 1.0).abs() < 1e-6);
    // 12 dB down on a 24 dB range.
    assert!((notes[1].velocity - 0.5).abs() < 0.05, "{notes:?}");
}

#[test]
fn the_pitch_range_limits_what_is_found() {
    let x = melody();
    let params = PitchParams {
        key_lo: 55,
        key_hi: 70,
        ..PitchParams::default()
    };
    let notes = extract_pitched_notes(&[&x], SR, &params);
    let keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
    assert_eq!(keys, vec![60, 64, 67], "{notes:?}");
}

#[test]
fn the_minimum_length_drops_short_notes() {
    let mut x = vec![0.0f32; frames(1.5)];
    note(&mut x, hz(60), 0.1, 0.5, 0.5);
    note(&mut x, hz(67), 0.7, 0.78, 0.5);
    let params = PitchParams {
        min_note_ms: 150,
        ..PitchParams::default()
    };
    let notes = extract_pitched_notes(&[&x], SR, &params);
    assert_eq!(notes.len(), 1, "{notes:?}");
    let kept = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    assert_eq!(kept.len(), 2, "{kept:?}");
}

#[test]
fn silence_and_garbage_give_no_notes_and_no_nan() {
    let silent = vec![0.0f32; frames(2.0)];
    assert!(extract_pitched_notes(&[&silent], SR, &PitchParams::default()).is_empty());
    assert!(extract_drum_notes(&[&silent], SR, &DrumParams::default()).is_empty());
    let mut garbage = melody();
    for (i, v) in garbage.iter_mut().enumerate() {
        match i % 7 {
            0 => *v = f32::NAN,
            3 => *v = f32::INFINITY,
            _ => {}
        }
    }
    for n in extract_pitched_notes(&[&garbage], SR, &PitchParams::default()) {
        assert!(n.velocity.is_finite() && n.velocity > 0.0);
    }
    for n in extract_drum_notes(&[&garbage], SR, &DrumParams::default()) {
        assert!(n.velocity.is_finite() && n.velocity > 0.0);
    }
    let nan = vec![f32::NAN; frames(1.0)];
    assert!(extract_pitched_notes(&[&nan], SR, &PitchParams::default()).is_empty());
    assert!(extract_pitched_notes(&[], SR, &PitchParams::default()).is_empty());
    assert!(extract_pitched_notes(&[&silent[..100]], SR, &PitchParams::default()).is_empty());
    let odd = PitchParams {
        sensitivity: f32::NAN,
        min_note_ms: 0,
        key_lo: 200,
        key_hi: 3,
        tolerance_cents: 0,
        bend: true,
    };
    let _ = extract_pitched_notes(&[&garbage], SR, &odd);
}

#[test]
fn white_noise_gives_no_pitched_notes() {
    let mut rng = Rng::new(7);
    let x: Vec<f32> = (0..frames(3.0))
        .map(|_| 0.5 * (rng.next_f32() * 2.0 - 1.0))
        .collect();
    for s in [0.0f32, 0.5, 1.0] {
        let params = PitchParams {
            sensitivity: s,
            ..PitchParams::default()
        };
        let notes = extract_pitched_notes(&[&x], SR, &params);
        assert!(notes.is_empty(), "sensitivity {s}: {notes:?}");
    }
}

#[test]
fn a_stereo_take_and_other_rates_give_the_same_notes() {
    let x = melody();
    let mono = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    let stereo = extract_pitched_notes(&[&x, &x], SR, &PitchParams::default());
    assert_eq!(mono, stereo);
    // The same melody at 44.1 kHz.
    let sr = 44_100u32;
    let mut y = vec![0.0f32; (3.0 * f64::from(sr)) as usize];
    for (key, from, to) in MELODY {
        let (a, b) = (
            (from * f64::from(sr)) as usize,
            (to * f64::from(sr)) as usize,
        );
        for (i, v) in y[a..b].iter_mut().enumerate() {
            let t = i as f64 / f64::from(sr);
            *v += (0.5 * (std::f64::consts::TAU * hz(key) * t).sin()) as f32;
        }
    }
    let notes = extract_pitched_notes(&[&y], sr, &PitchParams::default());
    let keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
    assert_eq!(keys, MELODY.map(|m| m.0).to_vec(), "{notes:?}");
}

#[test]
fn the_same_input_gives_the_same_notes() {
    let x = melody();
    let a = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    let b = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    assert_eq!(a, b);
}

// ------------------------------------------------------------------ drums

/// A kick: a sine sweeping from 150 Hz down to 50 Hz, decaying over about 120 ms.
fn kick(x: &mut [f32], at: f64, amp: f32) {
    let a = frames(at);
    let mut phase = 0.0f64;
    for i in 0..frames(0.4).min(x.len() - a) {
        let t = i as f64 / f64::from(SR);
        let f = 50.0 + 100.0 * (-t / 0.03).exp();
        phase += std::f64::consts::TAU * f / f64::from(SR);
        x[a + i] += amp * ((-t / 0.12).exp() * phase.sin()) as f32;
    }
}

/// A snare: a 190 Hz body and noise low-passed around 3 kHz, decaying over about 70 ms.
fn snare(x: &mut [f32], at: f64, amp: f32, rng: &mut Rng) {
    let a = frames(at);
    let k = (-std::f64::consts::TAU * 3000.0 / f64::from(SR)).exp();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for i in 0..frames(0.4).min(x.len() - a) {
        let t = i as f64 / f64::from(SR);
        let n = f64::from(rng.next_f32() * 2.0 - 1.0);
        s1 = n * (1.0 - k) + s1 * k;
        s2 = s1 * (1.0 - k) + s2 * k;
        let body = 0.3 * (std::f64::consts::TAU * 190.0 * t).sin() * (-t / 0.05).exp();
        let wires = 3.0 * s2 * (-t / 0.07).exp();
        x[a + i] += amp * (body + wires) as f32;
    }
}

/// A hat: high-passed noise (a second difference) decaying with time constant `tau` seconds.
fn hat(x: &mut [f32], at: f64, amp: f32, tau: f64, rng: &mut Rng) {
    let a = frames(at);
    let (mut p1, mut p2) = (0.0f64, 0.0f64);
    for i in 0..frames(tau * 8.0).min(x.len() - a) {
        let t = i as f64 / f64::from(SR);
        let n = f64::from(rng.next_f32() * 2.0 - 1.0);
        let hp = n - 2.0 * p1 + p2;
        (p2, p1) = (p1, n);
        x[a + i] += amp * (0.25 * hp * (-t / tau).exp()) as f32;
    }
}

fn drum_loop() -> (Vec<f32>, Vec<(f64, u8)>) {
    let mut rng = Rng::new(3);
    let mut x = vec![0.0f32; frames(4.0)];
    kick(&mut x, 0.10, 0.9);
    snare(&mut x, 0.50, 0.8, &mut rng);
    hat(&mut x, 0.90, 0.6, 0.012, &mut rng);
    hat(&mut x, 1.30, 0.6, 0.15, &mut rng);
    kick(&mut x, 1.90, 0.9);
    snare(&mut x, 2.30, 0.5, &mut rng);
    kick(&mut x, 2.70, 0.9);
    hat(&mut x, 2.70, 0.6, 0.012, &mut rng);
    hat(&mut x, 3.10, 0.4, 0.012, &mut rng);
    kick(&mut x, 3.40, 0.5);
    let want = vec![
        (0.10, 36),
        (0.50, 38),
        (0.90, 42),
        (1.30, 46),
        (1.90, 36),
        (2.30, 38),
        (2.70, 36),
        (2.70, 42),
        (3.10, 42),
        (3.40, 36),
    ];
    (x, want)
}

#[test]
fn a_drum_pattern_is_classified_with_positions_within_a_few_ms() {
    let (x, want) = drum_loop();
    let notes = extract_drum_notes(&[&x], SR, &DrumParams::default());
    let got: Vec<(f64, u8)> = notes.iter().map(|n| (secs(n.start), n.key)).collect();
    assert_eq!(got.len(), want.len(), "{got:?}");
    for (g, w) in got.iter().zip(&want) {
        assert_eq!(g.1, w.1, "{got:?}");
        assert!((g.0 - w.0).abs() < 0.006, "{got:?}");
    }
}

#[test]
fn drum_velocity_follows_the_hit_level_and_lengths_are_short() {
    let (x, _) = drum_loop();
    let notes = extract_drum_notes(&[&x], SR, &DrumParams::default());
    let kicks: Vec<&ExtractedNote> = notes.iter().filter(|n| n.key == 36).collect();
    let loud = kicks.first().expect("a kick");
    let soft = kicks.last().expect("a kick");
    assert!(loud.velocity > soft.velocity, "{kicks:?}");
    for n in &notes {
        assert!(n.length >= 1 && secs(n.length) <= 0.3 + 1e-9, "{n:?}");
        assert!(n.velocity > 0.0 && n.velocity <= 1.0);
    }
}

#[test]
fn without_the_open_hat_option_every_hat_is_closed() {
    let (x, _) = drum_loop();
    let params = DrumParams {
        open_hats: false,
        ..DrumParams::default()
    };
    let notes = extract_drum_notes(&[&x], SR, &params);
    assert!(notes.iter().all(|n| n.key != 46));
    assert_eq!(notes.iter().filter(|n| n.key == 42).count(), 4);
}

#[test]
fn a_steady_tone_is_not_taken_for_a_pattern_of_hits() {
    let mut x = vec![0.0f32; frames(2.0)];
    note(&mut x, hz(60), 0.2, 1.8, 0.5);
    let notes = extract_drum_notes(&[&x], SR, &DrumParams::default());
    assert!(notes.len() <= 1, "{notes:?}");
}

// ------------------------------------------------------------------ bend

fn with_bend() -> PitchParams {
    PitchParams {
        bend: true,
        ..PitchParams::default()
    }
}

#[test]
fn a_glide_is_kept_as_a_bend_contour() {
    // E4 sliding up by 0.6 semitone over 150 ms, then holding.
    let mut x = vec![0.0f32; frames(1.2)];
    let mut phase = 0.0f64;
    let (a, b) = (frames(0.1), frames(1.0));
    for (i, v) in x.iter_mut().enumerate().take(b).skip(a) {
        let t = (i - a) as f64 / f64::from(SR);
        let semis = 0.6 * (t / 0.15).min(1.0);
        phase += std::f64::consts::TAU * hz(64) * (semis / 12.0).exp2() / f64::from(SR);
        *v = 0.5 * phase.sin() as f32;
    }
    let notes = extract_pitched_notes(&[&x], SR, &with_bend());
    assert_eq!(notes.len(), 1, "{notes:?}");
    let n = &notes[0];
    let key = f32::from(n.key);
    assert!(n.bend.len() >= 2 && n.bend.len() < 30, "{:?}", n.bend);
    for w in n.bend.windows(2) {
        assert!(w[0].frame < w[1].frame);
    }
    for p in &n.bend {
        assert!(p.frame >= n.start && p.frame < n.start + n.length, "{p:?}");
    }
    // The note ends at 64.6 whatever key it was rounded to.
    let last = n.bend.last().expect("a point");
    assert!((key + last.semitones - 64.6).abs() < 0.1, "{n:?}");
    let first = n.bend.first().expect("a point");
    assert!(key + first.semitones < 64.35, "{n:?}");
    // Without the option the same note has no contour and the same key and timing.
    let plain = extract_pitched_notes(&[&x], SR, &PitchParams::default());
    assert!(plain[0].bend.is_empty());
    assert_eq!(
        (plain[0].key, plain[0].start, plain[0].length),
        (n.key, n.start, n.length)
    );
}

#[test]
fn a_steady_note_has_no_contour() {
    let x = melody();
    let notes = extract_pitched_notes(&[&x], SR, &with_bend());
    assert_eq!(notes.len(), MELODY.len());
    for n in &notes {
        assert!(n.bend.is_empty(), "{n:?}");
    }
}

#[test]
fn a_vibrato_is_kept_as_swings_of_the_contour() {
    // A4 with a 5.5 Hz vibrato of +-0.4 semitone.
    let mut x = vec![0.0f32; frames(1.6)];
    let mut phase = 0.0f64;
    let (a, b) = (frames(0.1), frames(1.5));
    for (i, v) in x.iter_mut().enumerate().take(b).skip(a) {
        let t = (i - a) as f64 / f64::from(SR);
        let semis = 0.4 * (std::f64::consts::TAU * 5.5 * t).sin();
        phase += std::f64::consts::TAU * hz(69) * (semis / 12.0).exp2() / f64::from(SR);
        *v = 0.5 * phase.sin() as f32;
    }
    let notes = extract_pitched_notes(&[&x], SR, &with_bend());
    assert_eq!(notes.len(), 1, "{notes:?}");
    let bend = &notes[0].bend;
    let top = bend.iter().fold(f32::MIN, |m, p| m.max(p.semitones));
    let bottom = bend.iter().fold(f32::MAX, |m, p| m.min(p.semitones));
    assert!(top > 0.25 && bottom < -0.25, "{bend:?}");
    assert!(top <= 2.0 && bottom >= -2.0);
    // Thinned: far fewer points than the 5 ms frames.
    assert!(bend.len() < 140 * 2 / 3, "{}", bend.len());
}

// ------------------------------------------------------------------ drum maps and kits

#[test]
fn the_pads_map_writes_the_classes_in_a_row_from_the_first_pad() {
    let (x, want) = drum_loop();
    let params = DrumParams {
        map: DrumMap::Pads,
        ..DrumParams::default()
    };
    let notes = extract_drum_notes(&[&x], SR, &params);
    assert_eq!(notes.len(), want.len());
    let pad = |gm: u8| match gm {
        36 => 36,
        38 => 37,
        42 => 38,
        _ => 39,
    };
    for (n, w) in notes.iter().zip(&want) {
        assert_eq!(n.key, pad(w.1), "{notes:?}");
    }
    assert_eq!(DrumMap::Pads.key(DrumClass::Crash), 43);
    assert_eq!(DrumMap::GeneralMidi.key(DrumClass::HighTom), 50);
}

/// A tom: a sine of `hz` decaying over about 150 ms.
fn tom(x: &mut [f32], at: f64, hz: f64, amp: f32) {
    let a = frames(at);
    for i in 0..frames(0.5).min(x.len() - a) {
        let t = i as f64 / f64::from(SR);
        x[a + i] += amp * ((-t / 0.15).exp() * (std::f64::consts::TAU * hz * t).sin()) as f32;
    }
}

/// A crash: high-passed noise decaying over about a second.
fn crash(x: &mut [f32], at: f64, amp: f32, rng: &mut Rng) {
    hat(x, at, amp, 0.4, rng);
}

fn full_kit() -> DrumParams {
    DrumParams {
        kit: DrumKit::Full,
        ..DrumParams::default()
    }
}

#[test]
fn the_full_kit_tells_toms_from_the_kick_and_a_crash_from_a_hat() {
    let mut rng = Rng::new(11);
    let mut x = vec![0.0f32; frames(6.0)];
    kick(&mut x, 0.10, 0.9);
    tom(&mut x, 0.70, 115.0, 0.9);
    tom(&mut x, 1.30, 160.0, 0.9);
    tom(&mut x, 1.90, 230.0, 0.9);
    snare(&mut x, 2.50, 0.8, &mut rng);
    hat(&mut x, 3.10, 0.6, 0.012, &mut rng);
    crash(&mut x, 3.50, 0.6, &mut rng);
    let notes = extract_drum_notes(&[&x], SR, &full_kit());
    let got: Vec<(f64, u8)> = notes.iter().map(|n| (secs(n.start), n.key)).collect();
    let want = [
        (0.10, 36),
        (0.70, 45),
        (1.30, 47),
        (1.90, 50),
        (2.50, 38),
        (3.10, 42),
        (3.50, 49),
    ];
    assert_eq!(got.len(), want.len(), "{got:?}");
    for (g, w) in got.iter().zip(want) {
        assert_eq!(g.1, w.1, "{got:?}");
        assert!((g.0 - w.0).abs() < 0.01, "{got:?}");
    }
    // The basic kit has no toms and no crash: they are a kick and an open hat.
    let basic = extract_drum_notes(&[&x], SR, &DrumParams::default());
    assert!(basic.iter().all(|n| ![45, 47, 49, 50].contains(&n.key)));
}

#[test]
fn the_full_kit_leaves_a_plain_loop_as_it_was() {
    let (x, want) = drum_loop();
    let notes = extract_drum_notes(&[&x], SR, &full_kit());
    let got: Vec<u8> = notes.iter().map(|n| n.key).collect();
    assert_eq!(got, want.iter().map(|w| w.1).collect::<Vec<_>>());
}

// ------------------------------------------------------------------ long takes

const LR: u32 = 16_000;

/// Mono audio at 16 kHz with sine notes `(key, from, to)` in seconds.
fn tones(seconds: f64, notes: &[(u8, f64, f64)]) -> Vec<f32> {
    let r = f64::from(LR);
    let mut x = vec![0.0f32; (seconds * r) as usize];
    for &(key, from, to) in notes {
        let (a, b) = ((from * r) as usize, ((to * r) as usize).min(x.len()));
        for (i, v) in x[a..b].iter_mut().enumerate() {
            let t = i as f64 / r;
            let env = ((i as f64 / 80.0).min(((b - a - i) as f64) / 80.0)).min(1.0);
            *v += 0.5 * (env * (std::f64::consts::TAU * hz(key) * t).sin()) as f32;
        }
    }
    x
}

fn read_from(x: &[f32], from: u64, n: u64) -> Vec<f32> {
    let a = (from as usize).min(x.len());
    x[a..(a + n as usize).min(x.len())].to_vec()
}

fn secs16(frames: u64) -> f64 {
    frames as f64 / f64::from(LR)
}

#[test]
fn a_short_take_is_analysed_in_one_pass_exactly_like_the_detector() {
    let x = tones(3.0, &[(60, 0.2, 0.8), (67, 1.0, 1.6)]);
    let params = PitchParams::default();
    let mut calls = Vec::new();
    let got = extract_in_segments(
        x.len() as u64,
        LR,
        Analysis::Pitched(&params),
        &mut |a, n| read_from(&x, a, n),
        &mut |done| {
            calls.push(done);
            true
        },
    )
    .expect("not stopped");
    assert_eq!(got, extract_pitched_notes(&[&x], LR, &params));
    assert_eq!(calls, vec![0, x.len() as u64]);
}

#[test]
fn a_long_take_is_analysed_whole_and_a_note_across_a_stretch_edge_is_kept_once() {
    // 150 s: a note every 10 s, and one that crosses the 60 s edge.
    let mut list: Vec<(u8, f64, f64)> = Vec::new();
    for i in 0..14 {
        let from = 5.0 + 10.0 * f64::from(i);
        if (55.0..65.0).contains(&from) {
            continue;
        }
        list.push((48 + (i % 7) as u8, from, from + 1.0));
    }
    list.push((72, 59.2, 61.0));
    list.sort_by(|a, b| a.1.total_cmp(&b.1));
    let x = tones(150.0, &list);
    let params = PitchParams::default();
    let mut calls = Vec::new();
    let got = extract_in_segments(
        x.len() as u64,
        LR,
        Analysis::Pitched(&params),
        &mut |a, n| read_from(&x, a, n),
        &mut |done| {
            calls.push(done);
            true
        },
    )
    .expect("not stopped");
    assert_eq!(calls, vec![0, 960_000, 1_920_000, 2_400_000]);
    assert_eq!(got.len(), list.len(), "{got:?}");
    for (n, (key, from, to)) in got.iter().zip(&list) {
        assert_eq!(n.key, *key, "{got:?}");
        assert!((secs16(n.start) - from).abs() < 0.04, "{got:?}");
        assert!((secs16(n.start + n.length) - to).abs() < 0.06, "{got:?}");
    }
    for w in got.windows(2) {
        assert!(w[0].start < w[1].start);
    }
}

#[test]
fn the_velocities_of_a_long_take_share_one_reference() {
    // A loud note in the first stretch and a soft one in the second: the soft one is softer.
    let mut x = tones(150.0, &[(60, 5.0, 6.0)]);
    let soft = tones(150.0, &[(60, 100.0, 101.0)]);
    for (a, b) in x.iter_mut().zip(&soft) {
        *a += 0.1 * b;
    }
    let params = PitchParams::default();
    let got = extract_in_segments(
        x.len() as u64,
        LR,
        Analysis::Pitched(&params),
        &mut |a, n| read_from(&x, a, n),
        &mut |_| true,
    )
    .expect("not stopped");
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[0].velocity > 0.9, "{got:?}");
    assert!(got[1].velocity < 0.5, "{got:?}");
}

#[test]
fn stopping_between_stretches_gives_nothing_and_reads_no_further() {
    let x = tones(200.0, &[(60, 5.0, 6.0)]);
    let params = PitchParams::default();
    let mut reads = 0;
    let mut seen = Vec::new();
    let got = extract_in_segments(
        x.len() as u64,
        LR,
        Analysis::Pitched(&params),
        &mut |a, n| {
            reads += 1;
            read_from(&x, a, n)
        },
        &mut |done| {
            seen.push(done);
            seen.len() < 3
        },
    );
    assert!(got.is_none());
    assert_eq!(seen, vec![0, 960_000, 1_920_000]);
    assert_eq!(reads, 2);
    // A stop before anything is read.
    let none = extract_in_segments(
        x.len() as u64,
        LR,
        Analysis::Pitched(&params),
        &mut |a, n| read_from(&x, a, n),
        &mut |_| false,
    );
    assert!(none.is_none());
}

#[test]
fn drums_are_analysed_in_stretches_too() {
    // Kicks at 5 s, 59.5 s (the edge) and 130 s.
    let mut x = vec![0.0f32; LR as usize * 150];
    for at in [5.0f64, 59.5, 130.0] {
        let a = (at * f64::from(LR)) as usize;
        let mut phase = 0.0f64;
        for i in 0..6_400 {
            let t = i as f64 / f64::from(LR);
            let f = 50.0 + 100.0 * (-t / 0.03).exp();
            phase += std::f64::consts::TAU * f / f64::from(LR);
            x[a + i] += (0.9 * (-t / 0.12).exp() * phase.sin()) as f32;
        }
    }
    let params = DrumParams::default();
    let got = extract_in_segments(
        x.len() as u64,
        LR,
        Analysis::Drums(&params),
        &mut |a, n| read_from(&x, a, n),
        &mut |_| true,
    )
    .expect("not stopped");
    let starts: Vec<f64> = got.iter().map(|n| secs16(n.start)).collect();
    assert_eq!(got.len(), 3, "{starts:?}");
    for (s, w) in starts.iter().zip([5.0, 59.5, 130.0]) {
        assert!((s - w).abs() < 0.01, "{starts:?}");
    }
    assert!(got.iter().all(|n| n.key == 36));
}
