//! Estimating the key of a song from its notes.
//!
//! The notes are counted into a histogram of the twelve pitch classes (a note counts as much as it
//! is long). The histogram is compared, by correlation, with the standard major and minor key
//! profiles turned to each of the twelve roots, and the best of the twenty-four keys wins. The
//! profiles are the published listener ratings of how well each pitch class fits a major or a minor
//! key. The result is a guess: [`KeyGuess::confidence`] says how clear it was.

use crate::harmony::Key;
use crate::song::{ClipContent, Song};
use crate::theory::ScaleKind;

/// How well each pitch class fits a major key, tonic first.
const MAJOR_PROFILE: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];

/// How well each pitch class fits a minor key, tonic first.
const MINOR_PROFILE: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// The key a set of notes points to, and how clearly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyGuess {
    /// The best key: major or natural minor.
    pub key: Key,
    /// The correlation of the notes with that key's profile, `-1..=1` (above 0.7 is a clear key).
    pub confidence: f64,
}

/// The correlation of two series of twelve values; `None` when either is flat.
fn correlation(a: &[f64; 12], b: &[f64; 12]) -> Option<f64> {
    let mean = |v: &[f64; 12]| v.iter().sum::<f64>() / 12.0;
    let (ma, mb) = (mean(a), mean(b));
    let (mut dot, mut va, mut vb) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        let (x, y) = (a[i] - ma, b[i] - mb);
        dot += x * y;
        va += x * x;
        vb += y * y;
    }
    (va > 0.0 && vb > 0.0).then(|| dot / (va * vb).sqrt())
}

/// The key the notes point to. `notes` are `(key, weight)` pairs (a weight is usually the length of
/// the note; negative weights count as 0). `None` when there is no weight at all. On a tie the
/// lowest root wins, and major before minor.
pub fn detect_key(notes: &[(u8, i64)]) -> Option<KeyGuess> {
    let mut hist = [0f64; 12];
    for (key, weight) in notes {
        hist[usize::from(key % 12)] += (*weight).max(0) as f64;
    }
    detect_key_from_profile(&hist)
}

/// The key a pitch-class profile points to (`profile[pc]`, C first: how much of each pitch class
/// sounds, from notes or from the chroma of audio). `None` when it is empty, flat or not finite.
/// On a tie the lowest root wins, and major before minor, so a profile that fits a major key and
/// its relative minor exactly as well is read as the major key.
pub fn detect_key_from_profile(profile: &[f64; 12]) -> Option<KeyGuess> {
    let mut hist = [0f64; 12];
    for (h, p) in hist.iter_mut().zip(profile) {
        *h = if p.is_finite() { p.max(0.0) } else { 0.0 };
    }
    if hist.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    let mut best: Option<KeyGuess> = None;
    for root in 0..12u8 {
        for (profile, scale) in [
            (&MAJOR_PROFILE, ScaleKind::Major),
            (&MINOR_PROFILE, ScaleKind::NaturalMinor),
        ] {
            // The profile turned so that its tonic sits on `root`.
            let mut turned = [0f64; 12];
            for (i, v) in profile.iter().enumerate() {
                turned[(i + usize::from(root)) % 12] = *v;
            }
            let Some(score) = correlation(&hist, &turned) else {
                continue;
            };
            if best.is_none_or(|b| score > b.confidence + 1e-12) {
                best = Some(KeyGuess {
                    key: Key::new(root, scale),
                    confidence: score,
                });
            }
        }
    }
    best
}

/// The `(key, weight)` pairs of every note in the song's note clips (the arrangement lanes and the
/// launcher slots): a note weighs its length in ticks, muted notes count for nothing.
pub fn song_note_weights(song: &Song) -> Vec<(u8, i64)> {
    let mut out = Vec::new();
    let mut add = |content: &ClipContent| {
        if let ClipContent::Notes(notes) = content {
            out.extend(notes.iter().filter(|n| !n.muted).map(|n| (n.key, n.length)));
        }
    };
    for track in &song.tracks {
        for lane in &track.lanes {
            for clip in lane.clips.iter().filter(|c| !c.muted) {
                add(&clip.content);
            }
        }
    }
    for (_, _, slot) in song.launcher.clips() {
        add(&slot.clip.content);
    }
    out
}

/// The key of every note clip of the song (see [`detect_key`]).
pub fn detect_song_key(song: &Song) -> Option<KeyGuess> {
    detect_key(&song_note_weights(song))
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::PPQ;

    /// Notes of a scale, one beat each, with the tonic and fifth held longer.
    fn scale_notes(root: u8, intervals: &[u8]) -> Vec<(u8, i64)> {
        intervals
            .iter()
            .map(|i| {
                let long = *i == 0 || *i == 7;
                (60 + root + i, if long { 2 * PPQ } else { PPQ })
            })
            .collect()
    }

    #[test]
    fn a_c_major_set_is_c_major() {
        let notes = scale_notes(0, &[0, 2, 4, 5, 7, 9, 11]);
        let g = detect_key(&notes).unwrap();
        assert_eq!(g.key, Key::new(0, ScaleKind::Major));
        assert!(g.confidence > 0.7, "{}", g.confidence);
    }

    #[test]
    fn an_a_minor_set_is_a_minor() {
        let notes = scale_notes(9, &[0, 2, 3, 5, 7, 8, 10]);
        let g = detect_key(&notes).unwrap();
        assert_eq!(g.key, Key::new(9, ScaleKind::NaturalMinor));
        assert!(g.confidence > 0.7, "{}", g.confidence);
    }

    #[test]
    fn every_major_and_minor_key_is_found_from_its_own_scale() {
        for root in 0..12u8 {
            let major = detect_key(&scale_notes(root, &[0, 2, 4, 5, 7, 9, 11])).unwrap();
            assert_eq!(major.key, Key::new(root, ScaleKind::Major), "major {root}");
            let minor = detect_key(&scale_notes(root, &[0, 2, 3, 5, 7, 8, 10])).unwrap();
            assert_eq!(
                minor.key,
                Key::new(root, ScaleKind::NaturalMinor),
                "minor {root}"
            );
        }
    }

    #[test]
    fn a_chord_progression_points_to_its_key() {
        // C, F, G, C in a C major song.
        let mut notes = Vec::new();
        for chord in [[0u8, 4, 7], [5, 9, 0], [7, 11, 2], [0, 4, 7]] {
            for k in chord {
                notes.push((48 + k, 4 * PPQ));
            }
        }
        assert_eq!(
            detect_key(&notes).unwrap().key,
            Key::new(0, ScaleKind::Major)
        );
    }

    #[test]
    fn longer_notes_count_for_more() {
        // A long C and G with a few short stray notes.
        let notes = [
            (60, 16 * PPQ),
            (67, 16 * PPQ),
            (64, 8 * PPQ),
            (61, 1),
            (66, 1),
        ];
        assert_eq!(detect_key(&notes).unwrap().key.root, 0);
    }

    #[test]
    fn a_profile_reads_like_the_notes_it_counts() {
        // An A minor chroma: A, C, E strong, the rest of the scale weaker.
        let mut profile = [0.0f64; 12];
        for (pc, w) in [
            (9, 4.0),
            (0, 3.0),
            (4, 3.5),
            (2, 1.0),
            (5, 1.0),
            (7, 1.5),
            (11, 0.8),
        ] {
            profile[pc] = w;
        }
        let g = detect_key_from_profile(&profile).unwrap();
        assert_eq!(g.key, Key::new(9, ScaleKind::NaturalMinor));
        assert_eq!(detect_key_from_profile(&[0.0; 12]), None);
        assert_eq!(detect_key_from_profile(&[f64::NAN; 12]), None);
    }

    #[test]
    fn no_notes_and_no_weight_give_no_key() {
        assert_eq!(detect_key(&[]), None);
        assert_eq!(detect_key(&[(60, 0), (62, -5)]), None);
    }

    #[test]
    fn a_flat_histogram_has_no_key() {
        let all: Vec<(u8, i64)> = (0..12).map(|k| (60 + k, PPQ)).collect();
        assert_eq!(detect_key(&all), None);
    }

    #[test]
    fn the_result_is_the_same_whatever_the_octave_and_order() {
        let a = scale_notes(2, &[0, 2, 4, 5, 7, 9, 11]);
        let mut b: Vec<(u8, i64)> = a.iter().map(|(k, w)| (k + 12, *w)).collect();
        b.reverse();
        assert_eq!(detect_key(&a), detect_key(&b));
    }
}
