//! Harmony from audio: the pitches that sound in each moment of a recording, folded into a chroma
//! for chord and key detection ([`chords`]) and tracked into notes for polyphonic audio to notes
//! ([`poly`]).
//!
//! **Algorithm** (lengths scale with the rate):
//! 1. *Frames:* the mono mix is cut into Hann windows of `size` frames centred every `hop`
//!    frames (frame `k` is centred on source frame `k * hop`, zero padded past both ends). Chords
//!    use a hop of about 46 ms and a window of about 186 ms ([`AnalysisParams::chords`]); notes a
//!    hop of about 10 ms and a window of about 93 ms ([`AnalysisParams::notes`]).
//! 2. *Spectrum and whitening:* a radix-2 FFT of each window, then spectral whitening: the
//!    magnitude is divided by the level of its third-octave band raised to `1 - 0.33`, so a loud
//!    bass or a bright top does not hide the rest (each band keeps a third of its level in dB).
//! 3. *Peaks:* the local maxima of the whitened magnitude that stand out of the window's
//!    side lobes (the largest within four bins) and are no more than 40 dB under the loudest peak, with their frequency and
//!    level refined by a parabola through the log magnitude. A peak is a fractional MIDI key, so
//!    the log-frequency axis runs at any resolution (the constant-Q-like bins are the keys).
//! 4. *Multi-pitch:* for each candidate key of the range the salience is the weighted sum of the
//!    peaks found at its first ten harmonics (within 0.35 of a semitone; the fundamental must be
//!    there, and mostly not taken by a voice already found; a candidate on a low harmonic of a
//!    voice already found counts half). The strongest candidate is a voice; its harmonics are taken out of the peaks (each by
//!    the smallest of its peak and the peaks of its two neighbours as heard, what a smooth harmonic envelope predicts, so a
//!    partial shared with a higher note stays for that note) and the search runs again, up to the voice count, while a candidate keeps a
//!    share of the first voice's salience that the sensitivity sets.
//! 5. *Tuning:* the search runs once at A = 440, the deviation of the voices' fundamentals from
//!    the nearest key is averaged on a circle (weighted by salience), and the search runs again
//!    with every peak shifted by it, so a recording at A = 432 or 30 cents sharp reads true.
//! 6. *Chroma:* each frame's voices add their salience to their pitch class, so a note's
//!    harmonics count for the note and never for the fifth or the third they would fall on;
//!    the bass chroma holds the lowest voice that is at least a third as strong as the frame's
//!    strongest.
//!
//! Everything is pure and deterministic and runs off the real-time thread (it allocates): only
//! IEEE `+ - * /`, `sqrt`, integer arithmetic, sorting and this crate's deterministic `log2`,
//! `exp2`, sine and arctangent, so the same samples give the same result on every platform.
//! Silence, non-finite samples and noise give no voices and never `NaN`.

pub mod chords;
pub mod poly;

pub use chords::{
    BeatChroma, ChordGuess, ChordOptions, ChordSpan, detect_chords, key_profile, pool_frames,
    recognise_chords,
};
pub use poly::{PolyParams, extract_poly_notes};

use crate::det_math::{exp2_det, log2_det};
use crate::det_trig::{cos_cycles, sin_cycles};
use crate::stretch::fft::{Fft, atan2_det};

/// Peak level of the whole take below which it is silent (about -80 dBFS).
const MIN_PEAK: f32 = 1.0e-4;
/// Window RMS below which a frame has no voices (about -70 dBFS).
const MIN_RMS: f32 = 3.0e-4;
/// A frame quieter than the loudest frame by more than this (dB) has no voices.
const LEVEL_RANGE_DB: f32 = 50.0;
/// Peaks this far (dB) below the loudest peak of a frame are not read.
const PEAK_RANGE_DB: f32 = 40.0;
/// A peak must be the largest magnitude within this many bins on each side (so the side lobes of
/// a Hann window, 31 dB down, never count).
const SIDE_BINS: usize = 4;
/// Harmonics summed by the salience.
const HARMONICS: usize = 10;
/// How far a peak may be from a harmonic, in semitones.
const HARMONIC_TOLERANCE: f32 = 0.35;
/// How far the fundamental may be from its key before the tuning is known, in semitones.
const FIRST_PASS_TOLERANCE: f32 = 0.5;
/// The share of its peak a candidate's fundamental must still hold after the voices found so far
/// are taken out.
const FUNDAMENTAL_LEFT: f32 = 0.4;
/// What the salience of a candidate on the 2nd to 6th harmonic of a voice already found is
/// multiplied by.
const HARMONIC_GHOST: f32 = 0.5;
/// Whitening exponent: the share of each band's level (in dB) that is kept.
const WHITEN_NU: f32 = 0.33;
/// The lowest band of the whitening, in Hz; bands are a third of an octave apart.
const WHITEN_LO_HZ: f64 = 40.0;
/// A voice must be at least this share of the frame's strongest voice to be the bass.
const BASS_SHARE: f32 = 0.3;
/// The most voices a frame holds.
pub const MAX_VOICES: usize = 6;

/// How the take is cut and what is looked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalysisParams {
    /// Frames between two analysis frames.
    pub hop: usize,
    /// Window length, a power of two.
    pub size: usize,
    /// The most voices per frame, `1..=6`.
    pub max_voices: usize,
    /// `0..=1`: more finds quieter voices (NaN gives 0.5).
    pub sensitivity: f32,
    /// Lowest key looked for.
    pub key_lo: u8,
    /// Highest key looked for.
    pub key_hi: u8,
}

/// The smallest power of two at least `n` (and at least 256).
fn pow2_at_least(n: f64) -> usize {
    let mut size = 256usize;
    while (size as f64) < n && size < (1 << 16) {
        size *= 2;
    }
    size
}

impl AnalysisParams {
    /// For chords and keys: a hop of about 46 ms and a window of about 186 ms (2048 and 8192
    /// frames at 44.1 kHz), six voices from E1 to C7.
    pub fn chords(rate: u32) -> Self {
        let r = f64::from(rate.max(1));
        Self {
            hop: ((r * 0.0464).round() as usize).max(1),
            size: pow2_at_least(r * 0.17),
            max_voices: MAX_VOICES,
            sensitivity: 0.5,
            key_lo: 28,
            key_hi: 96,
        }
    }

    /// For notes: a hop of about 10 ms and a window of about 93 ms (4096 frames at 44.1 kHz).
    pub fn notes(rate: u32, max_voices: usize, sensitivity: f32, key_lo: u8, key_hi: u8) -> Self {
        let r = f64::from(rate.max(1));
        Self {
            hop: ((r * 0.01).round() as usize).max(1),
            size: pow2_at_least(r * 0.085),
            max_voices: max_voices.clamp(1, MAX_VOICES),
            sensitivity,
            key_lo: key_lo.min(key_hi),
            key_hi: key_hi.max(key_lo),
        }
    }
}

/// One pitch found in a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Voice {
    /// MIDI key.
    pub key: u8,
    /// Salience (harmonic sum of the whitened peaks), > 0.
    pub strength: f32,
}

/// One analysed frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// RMS of the window (linear).
    pub level: f32,
    /// The voices, strongest first.
    pub voices: Vec<Voice>,
    /// The salience of each pitch class (C first).
    pub chroma: [f32; 12],
    /// The salience of the bass voice, at its pitch class.
    pub bass: [f32; 12],
}

/// The analysis of a take.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// The sample rate.
    pub rate: u32,
    /// Frames between two analysis frames (frame `k` is centred on source frame `k * hop`).
    pub hop: usize,
    /// The window length.
    pub size: usize,
    /// How far the recording is from A = 440, in cents (positive: sharp).
    pub tuning_cents: f32,
    /// The frames, in order.
    pub frames: Vec<Frame>,
}

impl Analysis {
    /// The source frame frame `k` is centred on.
    pub fn centre(&self, k: usize) -> u64 {
        (k * self.hop) as u64
    }
}

/// A spectral peak: a fractional MIDI key and a whitened level.
#[derive(Debug, Clone, Copy)]
struct Peak {
    pitch: f32,
    amp: f32,
}

/// The FFT, window and whitening bands of one window length.
struct Spectrum {
    fft: Fft,
    window: Vec<f64>,
    /// For each bin, the band below it and the weight of the band above (linear in log frequency).
    bin_band: Vec<(usize, f64)>,
    bands: usize,
    rate: f64,
    size: usize,
}

impl Spectrum {
    fn new(size: usize, rate: u32) -> Self {
        let window = (0..size)
            .map(|i| 0.5 - 0.5 * cos_cycles(i as f64 / size as f64))
            .collect();
        let rate = f64::from(rate);
        let nyquist = rate / 2.0;
        let mut bands = 1usize;
        while WHITEN_LO_HZ * f64::from(exp2_det(bands as f32 / 3.0)) < nyquist && bands < 40 {
            bands += 1;
        }
        let bin_band = (0..=size / 2)
            .map(|k| {
                let hz = (k as f64 * rate / size as f64).max(WHITEN_LO_HZ);
                let pos = 3.0 * f64::from(log2_det((hz / WHITEN_LO_HZ) as f32));
                let b = (pos.floor().max(0.0) as usize).min(bands - 1);
                let frac = (pos - b as f64).clamp(0.0, 1.0);
                (b, if b + 1 < bands { frac } else { 0.0 })
            })
            .collect();
        Self {
            fft: Fft::new(size),
            window,
            bin_band,
            bands,
            rate,
            size,
        }
    }

    /// The peaks of one window of samples (`x.len() == size`), sorted by pitch.
    fn peaks(&self, x: &[f32], re: &mut [f64], im: &mut [f64]) -> Vec<Peak> {
        for i in 0..self.size {
            re[i] = f64::from(x[i]) * self.window[i];
            im[i] = 0.0;
        }
        self.fft.run(re, im, false);
        let half = self.size / 2;
        let mut mag: Vec<f64> = (0..=half)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
            .collect();
        // Whitening: the mean power of each band (triangular in log frequency).
        let mut power = vec![0.0f64; self.bands];
        let mut weight = vec![0.0f64; self.bands];
        for (k, m) in mag.iter().enumerate().skip(1) {
            let (b, frac) = self.bin_band[k];
            power[b] += (1.0 - frac) * m * m;
            weight[b] += 1.0 - frac;
            if frac > 0.0 {
                power[b + 1] += frac * m * m;
                weight[b + 1] += frac;
            }
        }
        let floor = 1.0e-12;
        let gain: Vec<f64> = power
            .iter()
            .zip(&weight)
            .map(|(p, w)| {
                let sigma = if *w > 0.0 { (p / w).sqrt() } else { 0.0 } + floor;
                f64::from(exp2_det((WHITEN_NU - 1.0) * log2_det(sigma as f32)))
            })
            .collect();
        for (k, m) in mag.iter_mut().enumerate() {
            let (b, frac) = self.bin_band[k];
            let g = if frac > 0.0 {
                gain[b] * (1.0 - frac) + gain[b + 1] * frac
            } else {
                gain[b]
            };
            *m *= g;
        }
        let top = mag.iter().skip(1).fold(0.0f64, |a, b| a.max(*b));
        if top <= 0.0 {
            return Vec::new();
        }
        let gate = top * f64::from(exp2_det(-PEAK_RANGE_DB / 6.020_6));
        let mut out = Vec::new();
        for k in 2..half.saturating_sub(1) {
            let m = mag[k];
            if m <= gate || m < mag[k - 1] || m < mag[k + 1] {
                continue;
            }
            let lo = k.saturating_sub(SIDE_BINS);
            let hi = (k + SIDE_BINS).min(half);
            let local = mag[lo..=hi].iter().fold(0.0f64, |a, b| a.max(*b));
            if m < local {
                // Not the top of its neighbourhood: a side lobe or a shoulder.
                continue;
            }
            let (a, b, c) = (
                f64::from(log2_det(mag[k - 1] as f32)),
                f64::from(log2_det(m as f32)),
                f64::from(log2_det(mag[k + 1] as f32)),
            );
            let den = a - 2.0 * b + c;
            let delta = if den < 0.0 {
                (0.5 * (a - c) / den).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let hz = (k as f64 + delta) * self.rate / self.size as f64;
            if hz <= 0.0 {
                continue;
            }
            let level = exp2_det((b - 0.25 * (a - c) * delta) as f32);
            let pitch = 69.0 + 12.0 * log2_det((hz / 440.0) as f32);
            out.push(Peak { pitch, amp: level });
        }
        out
    }
}

/// `12 * log2(h)` for the harmonics `1..=HARMONICS`.
fn harmonic_offsets() -> [f32; HARMONICS] {
    let mut out = [0.0f32; HARMONICS];
    for (i, o) in out.iter_mut().enumerate() {
        *o = 12.0 * log2_det((i + 1) as f32);
    }
    out
}

/// The weight of harmonic `h` (1-based) in the salience.
fn harmonic_weight(h: usize) -> f32 {
    1.0 / (0.5 + 0.5 * h as f32)
}

/// The index of the strongest peak within `tol` of `pitch` (peaks sorted by pitch), by residual.
fn strongest_near(peaks: &[Peak], residual: &[f32], pitch: f32, tol: f32) -> Option<usize> {
    let from = peaks.partition_point(|p| p.pitch < pitch - tol);
    let mut best: Option<usize> = None;
    for (i, p) in peaks.iter().enumerate().skip(from) {
        if p.pitch > pitch + tol {
            break;
        }
        if residual[i] > 0.0 && best.is_none_or(|b| residual[i] > residual[b]) {
            best = Some(i);
        }
    }
    best
}

/// What the voice search of a frame found: the voices and, for the tuning, each voice's
/// fundamental peak pitch.
struct Found {
    voices: Vec<Voice>,
    fundamentals: Vec<(f32, f32)>,
}

/// The voices of one frame's peaks (see the module docs, step 4). `tuning` (semitones) is taken
/// off every peak; `first_tol` is the tolerance of the fundamental.
fn find_voices(peaks: &[Peak], p: &AnalysisParams, tuning: f32, first_tol: f32) -> Found {
    let offsets = harmonic_offsets();
    let tuned: Vec<Peak> = peaks
        .iter()
        .map(|q| Peak {
            pitch: q.pitch - tuning,
            amp: q.amp,
        })
        .collect();
    let mut residual: Vec<f32> = tuned.iter().map(|q| q.amp).collect();
    let s = crate::notes::clamp_sensitivity(p.sensitivity);
    let share = 0.4 - 0.25 * s;
    let mut found = Found {
        voices: Vec::new(),
        fundamentals: Vec::new(),
    };
    let mut first = 0.0f32;
    let mut used = [false; 128];
    for _ in 0..p.max_voices.clamp(1, MAX_VOICES) {
        // The best candidate: (key, salience, matched peak per harmonic).
        let mut best: Option<(u8, f32, [Option<usize>; HARMONICS])> = None;
        for key in p.key_lo..=p.key_hi.min(127) {
            if used[usize::from(key)] {
                continue;
            }
            let f0 = f32::from(key);
            let Some(h1) = strongest_near(&tuned, &residual, f0, first_tol) else {
                continue;
            };
            if residual[h1] < FUNDAMENTAL_LEFT * tuned[h1].amp {
                // The fundamental is mostly a harmonic of a voice already found.
                continue;
            }
            let mut matched = [None; HARMONICS];
            matched[0] = Some(h1);
            let mut sal = harmonic_weight(1) * residual[h1];
            for h in 2..=HARMONICS {
                if let Some(i) =
                    strongest_near(&tuned, &residual, f0 + offsets[h - 1], HARMONIC_TOLERANCE)
                {
                    matched[h - 1] = Some(i);
                    sal += harmonic_weight(h) * residual[i];
                }
            }
            // A key on a low harmonic of a voice already found (an octave, a twelfth...) is most
            // likely what is left of that voice: it counts for less.
            if found
                .voices
                .iter()
                .any(|v| matches!(key.checked_sub(v.key), Some(12 | 19 | 24 | 28 | 31)))
            {
                sal *= HARMONIC_GHOST;
            }
            if best.is_none_or(|b| sal > b.1) {
                best = Some((key, sal, matched));
            }
        }
        let Some((key, sal, matched)) = best else {
            break;
        };
        if found.voices.is_empty() {
            first = sal;
        } else if sal < first * share {
            break;
        }
        used[usize::from(key)] = true;
        found.voices.push(Voice { key, strength: sal });
        if let Some(i) = matched[0] {
            found
                .fundamentals
                .push((tuned[i].pitch - f32::from(key), sal));
        }
        // Take the voice's harmonics out by what a smooth envelope predicts.
        // The envelope as heard: the peaks at the harmonics before any voice took a share.
        let full = vec![1.0f32; tuned.len()];
        let amps: Vec<f32> = (0..HARMONICS)
            .map(|h| {
                let at = f32::from(key) + offsets[h];
                let tol = if h == 0 {
                    first_tol
                } else {
                    HARMONIC_TOLERANCE
                };
                strongest_near(&tuned, &full, at, tol).map_or(0.0, |i| tuned[i].amp)
            })
            .collect();
        for (h, m) in matched.iter().enumerate() {
            let Some(i) = *m else {
                continue;
            };
            let lo = h.saturating_sub(1);
            let hi = (h + 1).min(HARMONICS - 1);
            let smooth = amps[lo..=hi].iter().fold(f32::MAX, |m, a| m.min(*a));
            residual[i] = (residual[i] - smooth.min(residual[i])).max(0.0);
        }
    }
    found
}

/// The chroma and bass chroma of a frame's voices.
fn fold(voices: &[Voice]) -> ([f32; 12], [f32; 12]) {
    let mut chroma = [0.0f32; 12];
    let mut bass = [0.0f32; 12];
    let top = voices.iter().fold(0.0f32, |m, v| m.max(v.strength));
    for v in voices {
        chroma[usize::from(v.key % 12)] += v.strength;
    }
    if let Some(low) = voices
        .iter()
        .filter(|v| v.strength >= top * BASS_SHARE)
        .min_by_key(|v| v.key)
    {
        bass[usize::from(low.key % 12)] = low.strength;
    }
    (chroma, bass)
}

/// Frames of a take of `total` frames read a stretch at a time: `read(from, n)` gives the mono
/// audio of `[from, from + n)`; `progress(done)` is told how many source frames are finished and
/// returns `false` to stop (the result is then `None`).
pub fn analyse_in_segments(
    total: u64,
    rate: u32,
    p: &AnalysisParams,
    read: &mut dyn FnMut(u64, u64) -> Vec<f32>,
    progress: &mut dyn FnMut(u64) -> bool,
) -> Option<Analysis> {
    if !progress(0) {
        return None;
    }
    let (hop, size) = (p.hop.max(1), p.size.max(256).next_power_of_two());
    let count = (total as usize).div_ceil(hop);
    let spectrum = Spectrum::new(size, rate.max(1));
    let (mut re, mut im) = (vec![0.0f64; size], vec![0.0f64; size]);
    let mut window = vec![0.0f32; size];
    let mut levels: Vec<f32> = Vec::with_capacity(count);
    let mut peaks: Vec<Vec<Peak>> = Vec::with_capacity(count);
    let per = ((crate::notes::long::SEGMENT_SECONDS * u64::from(rate)) as usize / hop).max(1);
    let half = (size / 2) as i64;
    let mut k0 = 0usize;
    let mut loudest = 0.0f32;
    while k0 < count {
        let k1 = (k0 + per).min(count);
        let first = (k0 * hop) as i64 - half;
        let last = ((k1 - 1) * hop) as i64 + half;
        let from = first.max(0) as u64;
        let to = (last.max(0) as u64).min(total);
        let mut chunk = if to > from {
            read(from, to - from)
        } else {
            Vec::new()
        };
        for v in &mut chunk {
            if !v.is_finite() {
                *v = 0.0;
            }
        }
        loudest = loudest.max(chunk.iter().fold(0.0f32, |m, v| m.max(v.abs())));
        for k in k0..k1 {
            let start = (k * hop) as i64 - half;
            let mut sum = 0.0f64;
            for (i, w) in window.iter_mut().enumerate() {
                let at = start + i as i64 - from as i64;
                *w = if at >= 0 && (at as usize) < chunk.len() {
                    chunk[at as usize]
                } else {
                    0.0
                };
                sum += f64::from(*w) * f64::from(*w);
            }
            let level = (sum / size as f64).sqrt() as f32;
            levels.push(level);
            peaks.push(if level >= MIN_RMS {
                spectrum.peaks(&window, &mut re, &mut im)
            } else {
                Vec::new()
            });
        }
        k0 = k1;
        if !progress(((k1 * hop) as u64).min(total)) {
            return None;
        }
    }
    let top_level = levels.iter().fold(0.0f32, |m, v| m.max(*v));
    let gate = (top_level * exp2_det(-LEVEL_RANGE_DB / 6.020_6)).max(MIN_RMS);
    let silent = loudest < MIN_PEAK;
    let live = |k: usize| !silent && levels[k] >= gate;
    // First pass at A = 440 for the tuning.
    let (mut c, mut s) = (0.0f64, 0.0f64);
    for (k, frame) in peaks.iter().enumerate() {
        if !live(k) {
            continue;
        }
        for (dev, weight) in find_voices(frame, p, 0.0, FIRST_PASS_TOLERANCE).fundamentals {
            c += f64::from(weight) * cos_cycles(f64::from(dev));
            s += f64::from(weight) * sin_cycles(f64::from(dev));
        }
    }
    let tuning = if c * c + s * s > 0.0 {
        (atan2_det(s, c) / std::f64::consts::TAU) as f32
    } else {
        0.0
    };
    let frames = peaks
        .iter()
        .enumerate()
        .map(|(k, frame)| {
            let voices = if live(k) {
                find_voices(frame, p, tuning, HARMONIC_TOLERANCE).voices
            } else {
                Vec::new()
            };
            let (chroma, bass) = fold(&voices);
            Frame {
                level: levels[k],
                voices,
                chroma,
                bass,
            }
        })
        .collect();
    Some(Analysis {
        rate,
        hop,
        size,
        tuning_cents: tuning * 100.0,
        frames,
    })
}

/// The analysis of a whole take held in memory (the mean of the channels).
pub fn analyse(channels: &[&[f32]], rate: u32, p: &AnalysisParams) -> Analysis {
    let mono = crate::notes::mono_mix(channels);
    let mut read = |from: u64, n: u64| {
        let a = (from as usize).min(mono.len());
        let b = (a + n as usize).min(mono.len());
        mono[a..b].to_vec()
    };
    analyse_in_segments(mono.len() as u64, rate, p, &mut read, &mut |_| true).unwrap_or(Analysis {
        rate,
        hop: p.hop,
        size: p.size,
        tuning_cents: 0.0,
        frames: Vec::new(),
    })
}

#[cfg(test)]
mod tests;
