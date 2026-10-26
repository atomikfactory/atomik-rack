//! Music theory for the note editors: scales, scale degrees, chords and scale detection.
//!
//! The scale and chord types live in [`atomik_core::theory`] (so the devices can use them on the
//! audio thread) and are re-exported here unchanged. This module adds the helpers that return
//! `Vec`s and the scale detection, which the editors use off the audio thread.

pub use atomik_core::theory::*;

/// Turns offsets from `root` into keys, inverted `inversion` times, dropping keys above 127.
fn stack(root: u8, offsets: impl IntoIterator<Item = i32>, inversion: u8) -> Vec<u8> {
    let mut keys: Vec<i32> = offsets
        .into_iter()
        .map(|o| i32::from(root) + o)
        .filter(|k| (0..=i32::from(MAX_KEY)).contains(k))
        .collect();
    keys.sort_unstable();
    if keys.len() >= 3 {
        for _ in 0..usize::from(inversion).min(keys.len() - 1) {
            let low = keys.remove(0);
            keys.push(low + 12);
        }
    }
    let mut out: Vec<u8> = keys
        .into_iter()
        .filter(|k| *k <= i32::from(MAX_KEY))
        .map(|k| k as u8)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The keys of `kind` on `root`, ascending, in the given inversion (0 = root position).
pub fn chord_keys(root: u8, kind: ChordKind, inversion: u8) -> Vec<u8> {
    stack(
        root.min(MAX_KEY),
        kind.intervals().iter().map(|i| i32::from(*i)),
        inversion,
    )
}

/// The diatonic chord on `key` in `scale`: the key is first moved to the nearest in-scale key
/// (ties down), then thirds are stacked through the scale tones (`size.notes()` of them,
/// every second degree). Ascending, inverted `inversion` times.
pub fn diatonic_chord(scale: Scale, key: u8, size: DiatonicSize, inversion: u8) -> Vec<u8> {
    let root = scale.nearest(key.min(MAX_KEY));
    let mut keys = Vec::with_capacity(size.notes());
    for i in 0..size.notes() {
        match scale.step(root, 2 * i as i32) {
            Some(k) => keys.push(k),
            None => break,
        }
    }
    stack(
        keys.first().copied().unwrap_or(root),
        keys.iter().map(|k| i32::from(*k) - i32::from(keys[0])),
        inversion,
    )
}

/// The scale kinds [`detect_scale`] chooses from, in the order that breaks ties.
pub const DETECTABLE: [ScaleKind; 15] = [
    ScaleKind::Major,
    ScaleKind::NaturalMinor,
    ScaleKind::HarmonicMinor,
    ScaleKind::MelodicMinor,
    ScaleKind::Dorian,
    ScaleKind::Phrygian,
    ScaleKind::Lydian,
    ScaleKind::Mixolydian,
    ScaleKind::Locrian,
    ScaleKind::MajorPentatonic,
    ScaleKind::MinorPentatonic,
    ScaleKind::Blues,
    ScaleKind::WholeTone,
    ScaleKind::PhrygianDominant,
    ScaleKind::HungarianMinor,
];

/// The scale that fits the notes best. `notes` are `(key, weight)` pairs (a weight is usually
/// the length of the note; negative weights count as 0). The best scale has the least weight
/// outside it; among equals the one with the fewest tones, then the one whose root carries the
/// most weight, then the first of [`DETECTABLE`] and the lowest root. `None` when there is no
/// weight at all.
pub fn detect_scale(notes: &[(u8, i64)]) -> Option<Scale> {
    let mut hist = [0i64; 12];
    for (key, weight) in notes {
        hist[usize::from(key % 12)] += (*weight).max(0);
    }
    let total: i64 = hist.iter().sum();
    if total == 0 {
        return None;
    }
    type Score = (i64, usize, i64, usize, u8);
    let mut best: Option<(Score, Scale)> = None;
    for (order, kind) in DETECTABLE.into_iter().enumerate() {
        for root in 0..12u8 {
            let scale = Scale::new(root, kind);
            let mask = scale.pitch_mask();
            let outside: i64 = (0..12)
                .filter(|pc| mask & (1 << pc) == 0)
                .map(|pc| hist[pc])
                .sum();
            let score = (
                outside,
                kind.intervals().len(),
                -hist[usize::from(root)],
                order,
                root,
            );
            if best.is_none_or(|(b, _)| score < b) {
                best = Some((score, scale));
            }
        }
    }
    best.map(|(_, s)| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chord_has_its_intervals_and_inversions_rotate_the_bass() {
        let expect: [(ChordKind, &[u8]); 21] = [
            (ChordKind::Major, &[0, 4, 7]),
            (ChordKind::Minor, &[0, 3, 7]),
            (ChordKind::Dim, &[0, 3, 6]),
            (ChordKind::Aug, &[0, 4, 8]),
            (ChordKind::Sus2, &[0, 2, 7]),
            (ChordKind::Sus4, &[0, 5, 7]),
            (ChordKind::Sixth, &[0, 4, 7, 9]),
            (ChordKind::MinorSixth, &[0, 3, 7, 9]),
            (ChordKind::Seventh, &[0, 4, 7, 10]),
            (ChordKind::MajorSeventh, &[0, 4, 7, 11]),
            (ChordKind::MinorSeventh, &[0, 3, 7, 10]),
            (ChordKind::HalfDim, &[0, 3, 6, 10]),
            (ChordKind::DimSeventh, &[0, 3, 6, 9]),
            (ChordKind::Ninth, &[0, 4, 7, 10, 14]),
            (ChordKind::MajorNinth, &[0, 4, 7, 11, 14]),
            (ChordKind::MinorNinth, &[0, 3, 7, 10, 14]),
            (ChordKind::Add9, &[0, 4, 7, 14]),
            (ChordKind::Eleventh, &[0, 4, 7, 10, 14, 17]),
            (ChordKind::Thirteenth, &[0, 4, 7, 10, 14, 17, 21]),
            (ChordKind::Power, &[0, 7]),
            (ChordKind::Octave, &[0, 12]),
        ];
        assert_eq!(expect.len(), ChordKind::ALL.len());
        for (kind, offsets) in expect {
            assert_eq!(kind.intervals(), offsets, "{kind:?}");
            let want: Vec<u8> = offsets.iter().map(|o| 48 + o).collect();
            assert_eq!(chord_keys(48, kind, 0), want, "{kind:?}");
        }
        assert_eq!(chord_keys(60, ChordKind::Major, 1), [64, 67, 72]);
        assert_eq!(chord_keys(60, ChordKind::Major, 2), [67, 72, 76]);
        assert_eq!(chord_keys(60, ChordKind::Seventh, 3), [70, 72, 76, 79]);
        // More inversions than notes are limited to notes - 1.
        assert_eq!(chord_keys(60, ChordKind::Major, 3), [67, 72, 76]);
        assert_eq!(chord_keys(60, ChordKind::Major, 200), [67, 72, 76]);
        // Two-note chords have no inversions.
        assert_eq!(chord_keys(60, ChordKind::Power, 1), [60, 67]);
        assert_eq!(chord_keys(60, ChordKind::Octave, 2), [60, 72]);
    }

    #[test]
    fn chords_drop_the_notes_above_127_and_clamp_the_root() {
        assert_eq!(chord_keys(127, ChordKind::Major, 0), [127]);
        assert_eq!(chord_keys(124, ChordKind::Major, 0), [124]);
        assert_eq!(chord_keys(120, ChordKind::Major, 0), [120, 124, 127]);
        // The inverted bass would leave the range: dropped.
        assert_eq!(chord_keys(120, ChordKind::Major, 1), [124, 127]);
        assert_eq!(chord_keys(200, ChordKind::Power, 0), [127]);
        assert_eq!(chord_keys(0, ChordKind::Major, 0), [0, 4, 7]);
    }

    #[test]
    fn diatonic_triads_in_c_major_are_the_seven_classic_chords() {
        let s = Scale::new(0, ScaleKind::Major);
        let triad = |k| diatonic_chord(s, k, DiatonicSize::Triad, 0);
        assert_eq!(triad(60), [60, 64, 67], "I");
        assert_eq!(triad(62), [62, 65, 69], "ii");
        assert_eq!(triad(64), [64, 67, 71], "iii");
        assert_eq!(triad(65), [65, 69, 72], "IV");
        assert_eq!(triad(67), [67, 71, 74], "V");
        assert_eq!(triad(69), [69, 72, 76], "vi");
        assert_eq!(triad(71), [71, 74, 77], "vii dim");
    }

    #[test]
    fn diatonic_sevenths_and_ninths_in_c_major() {
        let s = Scale::new(0, ScaleKind::Major);
        let seventh = |k| diatonic_chord(s, k, DiatonicSize::Seventh, 0);
        assert_eq!(seventh(60), [60, 64, 67, 71], "Imaj7");
        assert_eq!(seventh(62), [62, 65, 69, 72], "ii7");
        assert_eq!(seventh(67), [67, 71, 74, 77], "V7");
        assert_eq!(seventh(71), [71, 74, 77, 81], "viim7b5");
        let ninth = |k| diatonic_chord(s, k, DiatonicSize::Ninth, 0);
        assert_eq!(ninth(60), [60, 64, 67, 71, 74]);
        assert_eq!(ninth(67), [67, 71, 74, 77, 81]);
    }

    #[test]
    fn diatonic_triads_in_a_natural_minor_and_in_other_roots() {
        let s = Scale::new(9, ScaleKind::NaturalMinor);
        let triad = |k| diatonic_chord(s, k, DiatonicSize::Triad, 0);
        assert_eq!(triad(57), [57, 60, 64], "i");
        assert_eq!(triad(59), [59, 62, 65], "ii dim");
        assert_eq!(triad(60), [60, 64, 67], "III");
        assert_eq!(triad(62), [62, 65, 69], "iv");
        assert_eq!(triad(64), [64, 67, 71], "v");
        assert_eq!(triad(65), [65, 69, 72], "VI");
        assert_eq!(triad(67), [67, 71, 74], "VII");
        let g = Scale::new(7, ScaleKind::Major);
        assert_eq!(
            diatonic_chord(g, 66, DiatonicSize::Triad, 0),
            [66, 69, 72],
            "F# dim in G"
        );
        // Harmonic minor III+ is augmented.
        let h = Scale::new(9, ScaleKind::HarmonicMinor);
        assert_eq!(diatonic_chord(h, 60, DiatonicSize::Triad, 0), [60, 64, 68]);
    }

    #[test]
    fn a_diatonic_chord_on_an_out_of_scale_key_uses_the_nearest_scale_key_and_inverts() {
        let s = Scale::new(0, ScaleKind::Major);
        assert_eq!(diatonic_chord(s, 61, DiatonicSize::Triad, 0), [60, 64, 67]);
        assert_eq!(diatonic_chord(s, 60, DiatonicSize::Triad, 1), [64, 67, 72]);
        assert_eq!(
            diatonic_chord(s, 60, DiatonicSize::Seventh, 2),
            [67, 71, 72, 76]
        );
    }

    #[test]
    fn diatonic_chords_at_the_top_of_the_range_drop_what_does_not_fit() {
        let s = Scale::new(0, ScaleKind::Major);
        assert_eq!(diatonic_chord(s, 127, DiatonicSize::Triad, 0), [127]);
        assert_eq!(
            diatonic_chord(s, 120, DiatonicSize::Triad, 0),
            [120, 124, 127]
        );
        let low = diatonic_chord(s, 0, DiatonicSize::Triad, 0);
        assert_eq!(low, [0, 4, 7]);
    }

    fn weights(keys: &[u8]) -> Vec<(u8, i64)> {
        keys.iter().map(|k| (*k, 1)).collect()
    }

    #[test]
    fn detection_finds_the_major_scale_from_its_notes() {
        let c = detect_scale(&weights(&[60, 62, 64, 65, 67, 69, 71, 72])).unwrap();
        assert_eq!(c, Scale::new(0, ScaleKind::Major));
        let d = detect_scale(&weights(&[62, 64, 66, 67, 69, 71, 73, 74, 74, 62])).unwrap();
        assert_eq!(d, Scale::new(2, ScaleKind::Major));
    }

    #[test]
    fn detection_uses_the_weight_of_the_root_to_tell_relative_modes_apart() {
        // The notes of C major with A the heaviest: A natural minor.
        let notes = [
            (57, 8),
            (60, 2),
            (62, 1),
            (64, 3),
            (65, 1),
            (67, 2),
            (71, 1),
        ];
        assert_eq!(
            detect_scale(&notes).unwrap(),
            Scale::new(9, ScaleKind::NaturalMinor)
        );
        // The same notes with C the heaviest: C major (also first in the tie order).
        let notes = [
            (60, 8),
            (62, 1),
            (64, 3),
            (65, 1),
            (67, 2),
            (69, 2),
            (71, 1),
        ];
        assert_eq!(
            detect_scale(&notes).unwrap(),
            Scale::new(0, ScaleKind::Major)
        );
        // D heaviest: D dorian.
        let notes = [
            (62, 8),
            (60, 1),
            (64, 1),
            (65, 1),
            (67, 2),
            (69, 2),
            (71, 1),
        ];
        assert_eq!(
            detect_scale(&notes).unwrap(),
            Scale::new(2, ScaleKind::Dorian)
        );
    }

    #[test]
    fn detection_picks_the_smallest_scale_that_holds_every_note() {
        // A minor pentatonic riff.
        let a = detect_scale(&[(57, 4), (60, 2), (62, 2), (64, 3), (67, 2)]).unwrap();
        assert_eq!(a, Scale::new(9, ScaleKind::MinorPentatonic));
        // Add the flat five: blues.
        let b = detect_scale(&[(57, 4), (60, 2), (62, 2), (63, 1), (64, 3), (67, 2)]).unwrap();
        assert_eq!(b.root, 9);
        assert!(b.contains(63));
        // A harmonic minor passage (raised seventh).
        let h = detect_scale(&[
            (57, 4),
            (59, 1),
            (60, 1),
            (62, 1),
            (64, 1),
            (65, 1),
            (68, 1),
        ])
        .unwrap();
        assert_eq!(h, Scale::new(9, ScaleKind::HarmonicMinor));
    }

    #[test]
    fn detection_of_nothing_is_none_and_of_a_mess_still_fits_the_most_weight() {
        assert_eq!(detect_scale(&[]), None);
        assert_eq!(detect_scale(&[(60, 0), (61, -4)]), None);
        // Every pitch class once: some 7-tone scale, with the root of the lowest-order tie.
        let all: Vec<(u8, i64)> = (60..72).map(|k| (k, 1)).collect();
        let s = detect_scale(&all).unwrap();
        assert_eq!(s.kind.intervals().len(), 7);
        // A single note is best covered by the pentatonic scale rooted on it.
        let one = detect_scale(&[(64, 5)]).unwrap();
        assert!(one.contains(64) && one.is_root(64));
    }

    /// Every scale on every root, against a plain reference written from the documented rules
    /// (the core implementation moved out of this crate; the editors must see no change).
    #[test]
    fn theory_core_matches_model_for_every_scale_and_key() {
        for kind in ScaleKind::ALL {
            for root in 0..12u8 {
                let scale = Scale::new(root, kind);
                let has = |k: i32| {
                    kind.intervals()
                        .iter()
                        .any(|i| i32::from(*i) == (k - i32::from(root)).rem_euclid(12))
                };
                for key in 0..=127u8 {
                    assert_eq!(
                        scale.contains(key),
                        has(i32::from(key)),
                        "{kind:?} {root} {key}"
                    );
                    // Nearest: the smallest distance wins, ties go down, 0..=127 only.
                    let want = (0..=127i32)
                        .filter(|k| has(*k))
                        .min_by_key(|k| ((k - i32::from(key)).abs(), *k))
                        .map_or(key, |k| k as u8);
                    assert_eq!(scale.nearest(key), want, "{kind:?} {root} nearest {key}");
                    for degrees in [-3i32, -1, 1, 2, 7] {
                        let mut k = i32::from(key);
                        let mut ok = true;
                        for _ in 0..degrees.abs() {
                            let next = if degrees > 0 {
                                (k + 1..=127).find(|c| has(*c))
                            } else {
                                (0..k).rev().find(|c| has(*c))
                            };
                            match next {
                                Some(n) => k = n,
                                None => ok = false,
                            }
                            if !ok {
                                break;
                            }
                        }
                        assert_eq!(
                            scale.step(key, degrees),
                            ok.then_some(k as u8),
                            "{kind:?} {root} step {key} {degrees}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn chord_into_matches_chord_keys_for_every_kind_and_inversion() {
        let mut out = [0u8; MAX_CHORD_KEYS];
        for kind in ChordKind::ALL {
            for root in 0..=127u8 {
                for inversion in 0..6u8 {
                    let n = chord_into(root, kind, inversion, &mut out);
                    assert_eq!(
                        &out[..n],
                        chord_keys(root, kind, inversion).as_slice(),
                        "{kind:?} {root} {inversion}"
                    );
                }
            }
        }
    }

    #[test]
    fn diatonic_chord_into_matches_diatonic_chord_for_every_scale_key_and_size() {
        let mut out = [0u8; MAX_CHORD_KEYS];
        for kind in ScaleKind::ALL {
            for root in 0..12u8 {
                let scale = Scale::new(root, kind);
                for size in DiatonicSize::ALL {
                    for key in 0..=127u8 {
                        for inversion in 0..4u8 {
                            let n = diatonic_chord_into(scale, key, size, inversion, &mut out);
                            assert_eq!(
                                &out[..n],
                                diatonic_chord(scale, key, size, inversion).as_slice(),
                                "{kind:?} {root} {size:?} {key} {inversion}"
                            );
                        }
                    }
                }
            }
        }
    }
}
