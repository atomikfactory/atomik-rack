//! "Will the bass read?": how much of the low notes of a track or of the master is still heard on a
//! small speaker, through the harmonics the note carries above its fundamental.
//!
//! A small speaker cannot play a 50 Hz fundamental. It can play the 2nd to 5th harmonics of the
//! note (100 to 250 Hz) and the ear rebuilds the pitch from them, so a bass reads when those
//! harmonics are there and loud enough. A pure sine has none and disappears.
//!
//! It runs on a worker from rendered audio, never on the audio thread. [`read`] takes the mid signal
//! `(L + R) / 2` and cuts it into Hann windows of [`FFT_SIZE`] frames every [`HOP`] frames (the
//! in-house radix-2 transform, deterministic, so the same audio reads the same on every platform to
//! within the rounding of one `f32` sum). In each window:
//!
//! 1. The note is the lowest strong peak of the power spectrum between [`LOW_MIN_HZ`] and
//!    [`LOW_MAX_HZ`] ("strong" is within 6 dB of the loudest bin of that range). A window whose low
//!    range holds less than [`MIN_POWER`] (-70 dB, a full-scale sine is 1) has no bass and is
//!    skipped.
//! 2. `fundamental` is the power within two bins of the peak. For each harmonic `k` from 2 to
//!    [`HARMONICS`] below the top of the spectrum, `harmonic[k]` is the power within two bins of
//!    `k` times the peak frequency.
//! 3. The note's energy is `fundamental + sum(harmonic)`. What survives is
//!    `sum(harmonic[k] * g[k])` with `g[k]` the power gain of the small-speaker lens at `k` times
//!    the frequency ([`crate::lens::response_db`]), so the filter and its bell decide what counts.
//!
//! The reading is `sum(survives) / sum(energy)` over the windows with a note: the share of the
//! low-end note energy that a small speaker still plays in the harmonics. Loud notes weigh more.
//! A saw bass at 55 Hz reads about 16 %, a pure sine under 0.1 %.

use crate::det_trig::cos_cycles;
use crate::lens::{Voicing, response_db};
use crate::spectral::Fft;

/// Frames of one analysis window.
pub const FFT_SIZE: usize = 8192;
/// Frames between two windows.
pub const HOP: usize = FFT_SIZE / 2;
/// The lowest note frequency looked for, Hz.
pub const LOW_MIN_HZ: f64 = 25.0;
/// The highest note frequency looked for, Hz.
pub const LOW_MAX_HZ: f64 = 150.0;
/// The highest harmonic counted.
pub const HARMONICS: usize = 5;
/// The power of the low range under which a window has no bass (a full-scale sine is 1).
pub const MIN_POWER: f64 = 1.0e-7;
/// The share (0 to 1) from which the bass reads.
pub const READS_FROM: f32 = 0.08;
/// The share (0 to 1) from which the bass reads in part.
pub const PARTLY_FROM: f32 = 0.02;
/// A peak counts as the note when it is within this ratio of the loudest bin of the range.
const PEAK_RATIO: f64 = 0.25;
/// The bins either side of a peak that belong to it (the main lobe of the window).
const LOBE: usize = 2;

/// What the reading says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// There are no low notes to judge.
    NoBass,
    /// The harmonics carry the note: it reads on a small speaker.
    Reads,
    /// Part of it reads: it is thin there.
    Partly,
    /// Almost nothing of it is left: the bass disappears.
    Lost,
}

impl Verdict {
    /// The sentence for the verdict, as the panel shows it.
    pub fn text(self) -> &'static str {
        match self {
            Self::NoBass => "No low notes to judge",
            Self::Reads => "Yes: the harmonics carry the bass",
            Self::Partly => "Partly: the bass is thin on a small speaker",
            Self::Lost => "No: the bass disappears on a small speaker",
        }
    }

    /// What to do about it, one line.
    pub fn advice(self) -> &'static str {
        match self {
            Self::NoBass => "Pick a track with a bass line, or the master of a song that has one.",
            Self::Reads => "Nothing to do.",
            Self::Partly => {
                "A little saturation or a layer with more harmonics would help it read."
            }
            Self::Lost => "Add harmonics to the bass: saturate it, or layer a saw above the sine.",
        }
    }
}

/// The result of [`read`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// The share of the low-end note energy that a small speaker plays in the harmonics, 0 to 1.
    pub share: f32,
    /// The windows that held a low note.
    pub windows: usize,
    /// The energy-weighted mean frequency of the low notes in Hz (0 without notes).
    pub note_hz: f32,
    /// The verdict.
    pub verdict: Verdict,
}

impl Reading {
    /// What a signal without low notes reads.
    pub const NONE: Self = Self {
        share: 0.0,
        windows: 0,
        note_hz: 0.0,
        verdict: Verdict::NoBass,
    };
}

/// The verdict for a share (0 to 1) of a signal that has notes.
pub fn verdict_of(share: f32) -> Verdict {
    if share >= READS_FROM {
        Verdict::Reads
    } else if share >= PARTLY_FROM {
        Verdict::Partly
    } else {
        Verdict::Lost
    }
}

/// The power within [`LOBE`] bins of bin `k`.
fn lobe(power: &[f64], k: usize) -> f64 {
    let from = k.saturating_sub(LOBE);
    let to = (k + LOBE + 1).min(power.len());
    power[from.min(to)..to].iter().sum()
}

/// The reading of a stereo signal at `rate` Hz (`right` may be empty for a mono one; the two are
/// read up to the shorter one when both are given). See the module docs.
pub fn read(left: &[f32], right: &[f32], rate: u32) -> Reading {
    let len = if right.is_empty() {
        left.len()
    } else {
        left.len().min(right.len())
    };
    let rate = rate.max(1);
    let per_bin = f64::from(rate) / FFT_SIZE as f64;
    let bins = FFT_SIZE / 2;
    let lo = (LOW_MIN_HZ / per_bin).ceil() as usize;
    let hi = ((LOW_MAX_HZ / per_bin).floor() as usize).min(bins - 1);
    if len == 0 || lo + 1 >= hi {
        return Reading::NONE;
    }
    // The small speaker's power gain at the first bins' harmonics is looked up per frequency.
    let gain = |hz: f64| 10f64.powf(response_db(Voicing::SmallSpeaker, hz, f64::from(rate)) / 10.0);
    let fft = Fft::new(FFT_SIZE);
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| (0.5 - 0.5 * cos_cycles(i as f64 / FFT_SIZE as f64)) as f32)
        .collect();
    let energy: f64 = window.iter().map(|w| f64::from(*w) * f64::from(*w)).sum();
    // A sine of amplitude A: the one-sided power sum is N * A^2 * sum(w^2) / 4.
    let scale = 4.0 / (FFT_SIZE as f64 * energy);
    let mut re = vec![0.0f32; FFT_SIZE];
    let mut im = vec![0.0f32; FFT_SIZE];
    let mut power = vec![0.0f64; bins];
    let sample = |i: usize| -> f32 {
        if i >= len {
            return 0.0;
        }
        let v = if right.is_empty() {
            left[i]
        } else {
            (left[i] + right[i]) * 0.5
        };
        if v.is_finite() { v } else { 0.0 }
    };
    let (mut total, mut survives, mut weighted_hz) = (0.0f64, 0.0f64, 0.0f64);
    let mut windows = 0usize;
    for b in 0..len.div_ceil(HOP) {
        let start = b * HOP;
        for (i, (r, w)) in re.iter_mut().zip(&window).enumerate() {
            *r = sample(start + i) * w;
        }
        im.fill(0.0);
        fft.forward(&mut re, &mut im);
        for (k, p) in power.iter_mut().enumerate() {
            *p =
                (f64::from(re[k]) * f64::from(re[k]) + f64::from(im[k]) * f64::from(im[k])) * scale;
        }
        let low: f64 = power[lo..=hi].iter().sum();
        if low < MIN_POWER {
            continue;
        }
        let loudest = power[lo..=hi].iter().copied().fold(0.0f64, f64::max);
        // The lowest strong local peak of the range is the note.
        let Some(peak) = (lo..=hi).find(|&k| {
            power[k] >= PEAK_RATIO * loudest
                && power[k] >= power[k - 1]
                && power[k] >= power[(k + 1).min(bins - 1)]
        }) else {
            continue;
        };
        // The frequency: the power-weighted centre of the bins around the peak.
        let near = &power[peak - 1..=peak + 1];
        let sum: f64 = near.iter().sum();
        let centre = peak as f64 - 1.0
            + near
                .iter()
                .enumerate()
                .map(|(i, p)| i as f64 * p)
                .sum::<f64>()
                / sum.max(f64::MIN_POSITIVE);
        let hz = centre * per_bin;
        let mut note = lobe(&power, peak);
        let mut kept = 0.0;
        for h in 2..=HARMONICS {
            let at = hz * h as f64;
            let k = (at / per_bin).round() as usize;
            if k + LOBE >= bins {
                break;
            }
            let p = lobe(&power, k);
            note += p;
            kept += p * gain(at);
        }
        total += note;
        survives += kept;
        weighted_hz += note * hz;
        windows += 1;
    }
    if windows == 0 || total <= 0.0 {
        return Reading::NONE;
    }
    let share = (survives / total) as f32;
    Reading {
        share,
        windows,
        note_hz: (weighted_hz / total) as f32,
        verdict: verdict_of(share),
    }
}

#[cfg(test)]
mod tests;
