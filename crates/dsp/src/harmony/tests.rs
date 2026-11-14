//! Tests of the harmony analysis on synthetic chords and voices with known pitches.

use super::*;
use atomik_core::theory::ChordKind;

const SR: u32 = 44_100;
/// 120 BPM.
const BEAT: f64 = 0.5;

fn frames(seconds: f64) -> usize {
    (seconds * f64::from(SR)).round() as usize
}

fn hz(key: f64) -> f64 {
    440.0 * ((key - 69.0) / 12.0).exp2()
}

/// A piano-like tone (eight partials falling as `1/h`, each decaying faster than the one below,
/// 4 ms attack, 20 ms release) of `key` (fractional) from `from` to `to` seconds, added to `x`.
fn tone(x: &mut [f32], key: f64, from: f64, to: f64, amp: f64, decay: f64) {
    let (a, b) = (frames(from), frames(to).min(x.len()));
    let (attack, release) = (frames(0.004) as f64, frames(0.02) as f64);
    let f0 = hz(key);
    for (i, v) in x[a..b].iter_mut().enumerate() {
        let t = i as f64 / f64::from(SR);
        let env = (i as f64 / attack)
            .min((b - a - i) as f64 / release)
            .min(1.0);
        let mut s = 0.0;
        for h in 1..=8 {
            let f = f0 * h as f64;
            if f > f64::from(SR) / 2.0 - 500.0 {
                break;
            }
            let fall = (-t * decay * (1.0 + 0.6 * h as f64)).exp();
            s += fall / h as f64 * (std::f64::consts::TAU * f * t).sin();
        }
        *v += (amp * env * s) as f32;
    }
}

/// A chord: root, quality, bass (when not the root) and the keys played.
struct Chord {
    root: u8,
    quality: ChordKind,
    bass: Option<u8>,
    keys: &'static [u8],
}

const fn ch(root: u8, quality: ChordKind, bass: Option<u8>, keys: &'static [u8]) -> Chord {
    Chord {
        root,
        quality,
        bass,
        keys,
    }
}

/// The chords, one bar of four beats each, struck on beats 1 and 3; `cents` detunes everything.
fn render(chords: &[Chord], cents: f64) -> Vec<f32> {
    let bar = 4.0 * BEAT;
    let mut x = vec![0.0f32; frames(bar * chords.len() as f64 + 0.5)];
    for (i, c) in chords.iter().enumerate() {
        for half in 0..2 {
            let from = i as f64 * bar + half as f64 * 2.0 * BEAT;
            for k in c.keys {
                tone(
                    &mut x,
                    f64::from(*k) + cents / 100.0,
                    from,
                    from + 2.0 * BEAT,
                    0.08,
                    0.6,
                );
            }
        }
    }
    x
}

fn beat_frames(count: usize) -> Vec<u64> {
    (0..count).map(|i| frames(i as f64 * BEAT) as u64).collect()
}

/// The share of beats whose chord (root, quality and, when `slash`, bass) is right, and the
/// lowest confidence of a chord found.
fn score(chords: &[Chord], cents: f64, opts: &ChordOptions) -> (f64, f32, Vec<String>) {
    let x = render(chords, cents);
    let a = analyse(&[&x], SR, &AnalysisParams::chords(SR));
    let beats = beat_frames(4 * chords.len());
    let found = detect_chords(&a, &beats, 1, opts);
    let mut right = 0usize;
    let mut log = Vec::new();
    for (i, b) in beats.iter().enumerate() {
        let want = &chords[i / 4];
        let got = found
            .iter()
            .find(|g| g.start_frame <= *b && *b < g.end_frame);
        let ok = got.is_some_and(|g| {
            g.root == want.root && g.quality == want.quality && (!opts.slash || g.bass == want.bass)
        });
        right += usize::from(ok);
        log.push(format!(
            "beat {i}: {:?}",
            got.map(|g| (g.root, g.quality, g.bass, g.confidence))
        ));
    }
    let low = found.iter().fold(1.0f32, |m, g| m.min(g.confidence));
    (right as f64 / beats.len() as f64, low, log)
}

const POP: [Chord; 4] = [
    ch(0, ChordKind::Major, None, &[36, 60, 64, 67]),
    ch(7, ChordKind::Major, None, &[43, 67, 71, 74]),
    ch(9, ChordKind::Minor, None, &[45, 69, 72, 76]),
    ch(5, ChordKind::Major, None, &[41, 65, 69, 72]),
];

#[test]
fn one_five_six_four_in_c_is_found_on_its_beats() {
    let (share, low, log) = score(&POP, 0.0, &ChordOptions::default());
    assert!(share >= 0.9, "{share} {log:#?}");
    assert!(low > 0.5, "{low} {log:#?}");
}

#[test]
fn a_recording_thirty_cents_sharp_still_reads_true() {
    let x = render(&POP, 30.0);
    let a = analyse(&[&x], SR, &AnalysisParams::chords(SR));
    assert!((a.tuning_cents - 30.0).abs() < 5.0, "{}", a.tuning_cents);
    let (share, low, log) = score(&POP, 30.0, &ChordOptions::default());
    assert!(share >= 0.9, "{share} {log:#?}");
    assert!(low > 0.5, "{low} {log:#?}");
}

#[test]
fn a_recording_at_a_432_reads_true() {
    let cents = 1200.0 * (432.0f64 / 440.0).log2();
    let (share, _, log) = score(&POP, cents, &ChordOptions::default());
    assert!(share >= 0.9, "{share} {log:#?}");
}

const MINOR: [Chord; 4] = [
    ch(9, ChordKind::Minor, None, &[45, 69, 72, 76]),
    ch(2, ChordKind::Minor, None, &[38, 62, 65, 69]),
    ch(4, ChordKind::Seventh, None, &[40, 64, 68, 71, 74]),
    ch(9, ChordKind::Minor, None, &[45, 69, 72, 76]),
];

#[test]
fn a_minor_progression_with_a_dominant_seventh_is_found() {
    let (share, low, log) = score(&MINOR, 0.0, &ChordOptions::default());
    assert!(share >= 0.9, "{share} {log:#?}");
    assert!(low > 0.5, "{low} {log:#?}");
}

#[test]
fn simple_mode_reads_the_seventh_as_its_triad() {
    let simple: [Chord; 4] = [
        ch(9, ChordKind::Minor, None, &[45, 69, 72, 76]),
        ch(2, ChordKind::Minor, None, &[38, 62, 65, 69]),
        ch(4, ChordKind::Major, None, &[40, 64, 68, 71, 74]),
        ch(9, ChordKind::Minor, None, &[45, 69, 72, 76]),
    ];
    let opts = ChordOptions {
        full: false,
        slash: true,
    };
    let (share, _, log) = score(&simple, 0.0, &opts);
    assert!(share >= 0.9, "{share} {log:#?}");
}

const SLASH: [Chord; 4] = [
    ch(0, ChordKind::Major, None, &[36, 60, 64, 67]),
    ch(0, ChordKind::Major, Some(7), &[43, 60, 64, 67]),
    ch(5, ChordKind::Major, None, &[41, 65, 69, 72]),
    ch(7, ChordKind::Major, None, &[43, 67, 71, 74]),
];

#[test]
fn a_c_over_g_gets_its_bass() {
    let (share, _, log) = score(&SLASH, 0.0, &ChordOptions::default());
    assert!(share >= 0.9, "{share} {log:#?}");
}

#[test]
fn without_slash_the_bass_is_left_out() {
    let x = render(&SLASH, 0.0);
    let a = analyse(&[&x], SR, &AnalysisParams::chords(SR));
    let opts = ChordOptions {
        full: true,
        slash: false,
    };
    let found = detect_chords(&a, &beat_frames(16), 1, &opts);
    assert!(!found.is_empty());
    assert!(found.iter().all(|g| g.bass.is_none()), "{found:?}");
}

#[test]
fn changes_per_bar_give_one_chord_per_bar() {
    let x = render(&POP, 0.0);
    let a = analyse(&[&x], SR, &AnalysisParams::chords(SR));
    let found = detect_chords(&a, &beat_frames(16), 4, &ChordOptions::default());
    let got: Vec<(u8, ChordKind)> = found.iter().map(|g| (g.root, g.quality)).collect();
    let want: Vec<(u8, ChordKind)> = POP.iter().map(|c| (c.root, c.quality)).collect();
    assert_eq!(got, want);
    let bar = frames(4.0 * BEAT) as u64;
    for (i, g) in found.iter().enumerate() {
        assert_eq!(g.start_frame, i as u64 * bar);
    }
}

#[test]
fn silence_has_no_chords() {
    let x = vec![0.0f32; frames(4.0)];
    let a = analyse(&[&x], SR, &AnalysisParams::chords(SR));
    assert!(a.frames.iter().all(|f| f.voices.is_empty()));
    assert!(detect_chords(&a, &beat_frames(8), 1, &ChordOptions::default()).is_empty());
    assert_eq!(key_profile(&a), [0.0; 12]);
}

#[test]
fn a_silent_bar_is_a_gap_between_chords() {
    let chords = [
        ch(0, ChordKind::Major, None, &[36, 60, 64, 67]),
        ch(0, ChordKind::Major, None, &[]),
        ch(7, ChordKind::Major, None, &[43, 67, 71, 74]),
    ];
    let x = render(&chords, 0.0);
    let a = analyse(&[&x], SR, &AnalysisParams::chords(SR));
    let found = detect_chords(&a, &beat_frames(12), 4, &ChordOptions::default());
    let got: Vec<(u64, u8)> = found.iter().map(|g| (g.start_frame, g.root)).collect();
    let bar = frames(4.0 * BEAT) as u64;
    assert_eq!(got, vec![(0, 0), (2 * bar, 7)], "{found:?}");
}

#[test]
fn non_finite_samples_give_no_nan() {
    let mut x = render(&POP[..2], 0.0);
    for i in (0..x.len()).step_by(997) {
        x[i] = if i % 2 == 0 { f32::NAN } else { f32::INFINITY };
    }
    let a = analyse(&[&x], SR, &AnalysisParams::chords(SR));
    assert!(a.tuning_cents.is_finite());
    for f in &a.frames {
        assert!(f.level.is_finite());
        assert!(f.chroma.iter().chain(&f.bass).all(|v| v.is_finite()));
        assert!(f.voices.iter().all(|v| v.strength.is_finite()));
    }
    for g in detect_chords(&a, &beat_frames(8), 1, &ChordOptions::default()) {
        assert!(g.confidence.is_finite());
    }
    let notes = extract_poly_notes(&[&x], SR, &PolyParams::default());
    assert!(
        notes
            .iter()
            .all(|n| n.velocity.is_finite() && n.level.is_finite())
    );
}

#[test]
fn a_stop_ends_the_analysis_with_nothing() {
    let x = render(&POP, 0.0);
    let mut read = |from: u64, n: u64| {
        let a = from as usize;
        x[a..(a + n as usize).min(x.len())].to_vec()
    };
    let mut calls = 0;
    let mut stop = |_done: u64| {
        calls += 1;
        calls < 2
    };
    let p = AnalysisParams::chords(SR);
    assert!(analyse_in_segments(x.len() as u64, SR, &p, &mut read, &mut stop).is_none());
    assert!(analyse_in_segments(x.len() as u64, SR, &p, &mut read, &mut |_| false).is_none());
}

#[test]
fn the_analysis_is_the_same_read_in_one_go_or_in_stretches() {
    let x = render(&POP, 0.0);
    let p = AnalysisParams::chords(SR);
    let whole = analyse(&[&x], SR, &p);
    let mut read = |from: u64, n: u64| {
        let a = from as usize;
        x[a..(a + n as usize).min(x.len())].to_vec()
    };
    let mut seen = Vec::new();
    let streamed = analyse_in_segments(x.len() as u64, SR, &p, &mut read, &mut |d| {
        seen.push(d);
        true
    })
    .unwrap();
    assert_eq!(whole, streamed);
    assert_eq!(seen.first(), Some(&0));
    assert_eq!(seen.last(), Some(&(x.len() as u64)));
}

#[test]
fn two_runs_are_bit_identical() {
    let x = render(&MINOR, 30.0);
    let p = AnalysisParams::chords(SR);
    assert_eq!(analyse(&[&x], SR, &p), analyse(&[&x], SR, &p));
    let y = counterpoint();
    let q = PolyParams::default();
    assert_eq!(
        extract_poly_notes(&[&y], SR, &q),
        extract_poly_notes(&[&y], SR, &q)
    );
}

// ---------------------------------------------------------------- polyphonic notes

/// A sustained tone of six harmonics (`1/h`) with a slow decay, 5 ms fades.
fn voice(x: &mut [f32], key: u8, from: f64, to: f64) {
    tone(x, f64::from(key), from, to, 0.12, 0.3);
}

fn secs(f: u64) -> f64 {
    f as f64 / f64::from(SR)
}

/// Checks `notes` against `want` (key, start seconds): the same keys, starts within 30 ms.
fn check_notes(notes: &[crate::notes::ExtractedNote], want: &[(u8, f64)]) {
    let got: Vec<(u8, f64)> = notes.iter().map(|n| (n.key, secs(n.start))).collect();
    assert_eq!(notes.len(), want.len(), "{got:?}");
    for (key, start) in want {
        assert!(
            got.iter()
                .any(|(k, s)| k == key && (s - start).abs() <= 0.03),
            "{key} at {start}: {got:?}"
        );
    }
}

#[test]
fn a_three_note_chord_gives_its_three_keys() {
    let mut x = vec![0.0f32; frames(1.6)];
    for k in [60, 64, 67] {
        voice(&mut x, k, 0.2, 1.2);
    }
    let notes = extract_poly_notes(&[&x], SR, &PolyParams::default());
    check_notes(&notes, &[(60, 0.2), (64, 0.2), (67, 0.2)]);
    for n in &notes {
        assert!((secs(n.start + n.length) - 1.2).abs() < 0.1, "{n:?}");
    }
}

fn counterpoint() -> Vec<f32> {
    let mut x = vec![0.0f32; frames(1.9)];
    for (key, from, to) in COUNTERPOINT {
        voice(&mut x, key, from, to);
    }
    x
}

const COUNTERPOINT: [(u8, f64, f64); 5] = [
    (72, 0.10, 0.60),
    (74, 0.60, 1.10),
    (76, 1.10, 1.60),
    (48, 0.10, 0.85),
    (55, 0.85, 1.60),
];

#[test]
fn two_voices_in_counterpoint_give_their_notes() {
    let notes = extract_poly_notes(&[&counterpoint()], SR, &PolyParams::default());
    let want: Vec<(u8, f64)> = COUNTERPOINT.iter().map(|(k, s, _)| (*k, *s)).collect();
    check_notes(&notes, &want);
}

#[test]
fn the_voice_count_caps_the_notes_at_once() {
    let mut x = vec![0.0f32; frames(1.6)];
    for k in [60, 64, 67] {
        voice(&mut x, k, 0.2, 1.2);
    }
    let p = PolyParams {
        max_voices: 1,
        ..PolyParams::default()
    };
    let notes = extract_poly_notes(&[&x], SR, &p);
    assert!(!notes.is_empty());
    for n in &notes {
        let at_once = notes
            .iter()
            .filter(|m| m.start < n.start + n.length && n.start < m.start + m.length)
            .count();
        assert_eq!(at_once, 1, "{notes:?}");
    }
}

#[test]
fn silence_gives_no_poly_notes() {
    let x = vec![0.0f32; frames(2.0)];
    assert!(extract_poly_notes(&[&x], SR, &PolyParams::default()).is_empty());
    assert!(extract_poly_notes(&[], SR, &PolyParams::default()).is_empty());
}

#[test]
fn the_stretch_runner_reads_polyphonic_notes() {
    let mut x = vec![0.0f32; frames(1.6)];
    for k in [60, 64, 67] {
        voice(&mut x, k, 0.2, 1.2);
    }
    let p = PolyParams::default();
    let mut read = |from: u64, n: u64| {
        let a = from as usize;
        x[a..(a + n as usize).min(x.len())].to_vec()
    };
    let notes = crate::notes::extract_in_segments(
        x.len() as u64,
        SR,
        crate::notes::Analysis::Poly(&p),
        &mut read,
        &mut |_| true,
    )
    .unwrap();
    assert_eq!(notes, extract_poly_notes(&[&x], SR, &p));
}

// ---- the chord of one sound -------------------------------------------------------------

fn held(keys: &[u8]) -> Vec<f32> {
    let mut x = vec![0.0f32; frames(1.5)];
    for k in keys {
        tone(&mut x, f64::from(*k), 0.0, 1.4, 0.1, 0.5);
    }
    x
}

#[test]
fn a_three_note_chord_is_found_with_its_notes_and_name() {
    let x = held(&[57, 60, 64]);
    let c = detect_sound_chord(&[&x], SR).expect("a chord");
    assert_eq!(c.keys, [57, 60, 64]);
    assert_eq!((c.root, c.quality, c.bass), (9, ChordKind::Minor, None));
    assert!(c.confidence > 0.5, "{}", c.confidence);
    assert_eq!(detect_sound_chord(&[&x], SR), Some(c), "deterministic");
}

#[test]
fn an_inversion_names_its_bass() {
    let x = held(&[52, 55, 60]);
    let c = detect_sound_chord(&[&x], SR).expect("a chord");
    assert_eq!(c.keys, [52, 55, 60]);
    assert_eq!((c.root, c.quality, c.bass), (0, ChordKind::Major, Some(4)));
}

#[test]
fn a_single_note_an_octave_and_silence_are_not_chords() {
    for keys in [&[57u8][..], &[45, 57][..], &[48][..]] {
        let x = held(keys);
        assert_eq!(detect_sound_chord(&[&x], SR), None, "{keys:?}");
    }
    assert_eq!(detect_sound_chord(&[&vec![0.0f32; 20_000]], SR), None);
    assert_eq!(detect_sound_chord(&[&[]], SR), None);
}
