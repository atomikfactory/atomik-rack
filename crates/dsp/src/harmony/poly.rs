//! Polyphonic audio to notes: the voices of the multi-pitch analysis tracked into notes.
//!
//! **Method.** The take is analysed with the note settings ([`AnalysisParams::notes`]: a hop of
//! about 10 ms, a window of about 93 ms, the voice count and sensitivity of [`PolyParams`]). For
//! each key, the frames where it is a voice make runs; gaps up to [`BRIDGE_MS`] are bridged. The
//! window sees a note before it starts and after it ends, so a run's start is placed a sixth of a
//! window after its first frame and its end a sixth of a window before its last, and the start
//! then moves to the nearest amplitude onset ([`crate::onset::detect_onsets`]) within half a
//! window. An onset inside a run where the key's salience rises by a third splits it: the same
//! key struck again. Runs shorter than the minimum note are dropped, and a note of a key ends
//! where the next of that key starts. The velocity follows the strongest salience of the note in
//! dB, relative to the loudest note of the take.
//!
//! **Limits.** This makes usable sketches of a piano or a guitar part, not a transcription: a
//! dense mix, a distorted guitar (whose harmonics are not those of one pitch) or two notes an
//! octave apart in the same instrument can lose a voice or add a ghost one.

use super::{AnalysisParams, analyse_in_segments};
use crate::notes::{ExtractedNote, clamp_sensitivity, mono_mix, velocity_for};
use crate::onset::detect_onsets;

/// The longest gap inside a note, in ms.
pub const BRIDGE_MS: u64 = 30;
/// How much a key's salience must rise at an onset to strike it again.
const RESTRIKE_RISE: f32 = 1.33;

/// What to extract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolyParams {
    /// `0..=1`: more finds quieter voices (NaN gives 0.5).
    pub sensitivity: f32,
    /// The most notes at once, `1..=6`.
    pub max_voices: usize,
    /// Notes shorter than this are dropped, in ms.
    pub min_note_ms: u32,
    /// Lowest key looked for.
    pub key_lo: u8,
    /// Highest key looked for.
    pub key_hi: u8,
}

impl Default for PolyParams {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            max_voices: 4,
            min_note_ms: 60,
            key_lo: 28,
            key_hi: 96,
        }
    }
}

/// The notes of the take `channels` at `rate` (see the module docs), sorted by start and key.
pub fn extract_poly_notes(channels: &[&[f32]], rate: u32, p: &PolyParams) -> Vec<ExtractedNote> {
    let mono = mono_mix(channels);
    if mono.is_empty() || rate == 0 {
        return Vec::new();
    }
    let s = clamp_sensitivity(p.sensitivity);
    let params = AnalysisParams::notes(rate, p.max_voices, s, p.key_lo, p.key_hi);
    let mut read = |from: u64, n: u64| {
        let a = (from as usize).min(mono.len());
        let b = (a + n as usize).min(mono.len());
        mono[a..b].to_vec()
    };
    let Some(a) = analyse_in_segments(mono.len() as u64, rate, &params, &mut read, &mut |_| true)
    else {
        return Vec::new();
    };
    let onsets = detect_onsets(&[&mono], rate, s);
    let (hop, size) = (a.hop as i64, a.size as i64);
    let total = mono.len() as i64;
    let ms = |m: u64| (m * u64::from(rate) / 1000) as i64;
    let bridge = (ms(BRIDGE_MS) / hop).max(1) as usize;
    let min_len = ms(u64::from(p.min_note_ms)).max(1);
    let count = a.frames.len();
    // The salience of each key in each frame.
    let strength = |key: u8, k: usize| {
        a.frames[k]
            .voices
            .iter()
            .find(|v| v.key == key)
            .map_or(0.0, |v| v.strength)
    };
    let mean = |key: u8, from: i64, to: i64| {
        let (lo, hi) = (
            (from.max(0) / hop) as usize,
            ((to.max(0) / hop) as usize).min(count),
        );
        if hi <= lo {
            return 0.0;
        }
        (lo..hi).map(|k| strength(key, k)).sum::<f32>() / (hi - lo) as f32
    };
    let peak_in = |key: u8, from: i64, to: i64| {
        let (lo, hi) = (
            (from.max(0) / hop) as usize,
            ((to.max(0) / hop) as usize + 1).min(count),
        );
        (lo..hi).fold(0.0f32, |m, k| m.max(strength(key, k)))
    };
    let nearest_onset = |at: i64, reach: i64| {
        onsets
            .iter()
            .map(|o| *o as i64)
            .filter(|o| (o - at).abs() <= reach)
            .min_by_key(|o| ((o - at).abs(), *o))
    };
    let mut notes: Vec<ExtractedNote> = Vec::new();
    for key in params.key_lo..=params.key_hi.min(127) {
        let mut k = 0usize;
        while k < count {
            if strength(key, k) <= 0.0 {
                k += 1;
                continue;
            }
            // A run: frames k..=last with gaps up to `bridge`.
            let first = k;
            let mut last = k;
            let mut j = k;
            while j < count && j <= last + bridge {
                let v = strength(key, j);
                if v > 0.0 {
                    last = j;
                }
                j += 1;
            }
            k = last + 1;
            let guess = first as i64 * hop + size / 6;
            let start = nearest_onset(guess, size / 2)
                .unwrap_or(guess)
                .clamp(0, total);
            let end = (last as i64 * hop - size / 6).clamp(start + 1, total.max(start + 1));
            // Split where the key is struck again.
            let mut cuts = vec![start];
            for o in onsets.iter().map(|o| *o as i64) {
                let from = *cuts.last().unwrap_or(&start);
                if o - from < min_len || end - o < min_len {
                    continue;
                }
                let before = mean(key, o - size / 2 - 3 * hop, o - size / 2);
                let after = mean(key, o + size / 6, o + size / 2);
                if after > before * RESTRIKE_RISE {
                    cuts.push(o);
                }
            }
            cuts.push(end);
            for w in cuts.windows(2) {
                let (from, to) = (w[0], w[1]);
                if to - from < min_len {
                    continue;
                }
                let level = peak_in(key, from - size / 6, to + size / 6);
                notes.push(ExtractedNote {
                    start: from as u64,
                    length: (to - from) as u64,
                    key,
                    velocity: 1.0,
                    level,
                    bend: Vec::new(),
                });
            }
        }
    }
    notes.sort_by_key(|n| (n.start, n.key));
    // A note ends where the next of its key starts.
    for i in 0..notes.len() {
        let (key, start) = (notes[i].key, notes[i].start);
        if let Some(next) = notes[i + 1..]
            .iter()
            .find(|n| n.key == key && n.start > start)
            .map(|n| n.start)
            && start + notes[i].length > next
        {
            notes[i].length = next - start;
        }
    }
    let reference = notes.iter().fold(0.0f32, |m, n| m.max(n.level));
    for n in &mut notes {
        n.velocity = velocity_for(n.level, reference);
    }
    notes
}
