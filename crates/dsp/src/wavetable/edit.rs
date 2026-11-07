//! Editing a wavetable: the maths behind the table editor.
//!
//! A table is edited as a list of **key frames** (one cycle of [`FRAME_LEN`] samples each, at
//! most [`MAX_KEYS`]); [`bake`] turns the list into the [`FRAMES`] frames the synth stores,
//! putting the key frames at evenly spaced positions and filling the frames between them with the
//! linear blend the synth plays between two stored frames (so a table of four key frames morphs
//! smoothly over the whole position range instead of holding each frame for a quarter of it).
//!
//! * **Drawing.** [`paint_line`] sets the samples under a stroke of the pointer; [`smooth`],
//!   [`normalise`], [`remove_dc`], [`invert`] and [`reverse`] are whole-frame processes.
//! * **Harmonics.** A frame is read as harmonics ([`analyse`]: a level and a phase for each of
//!   the first harmonics) and built from them ([`rebuild`], additive synthesis). [`set_harmonic`]
//!   swaps one harmonic of a frame for another and leaves every other one (also the ones above
//!   the bins on screen) exactly as it was. A [`Bin`]'s `level` is the amplitude of its sine and
//!   its `phase` is in cycles of that harmonic, so the harmonic is
//!   `level * sin(2 pi (h t + phase))`.
//! * **Phase-safe morph.** Two frames whose fundamentals are out of phase cancel it half way
//!   through a linear blend. [`align_fundamental`] shifts a frame in time until its fundamental
//!   has phase 0 (every harmonic `h` turns by `h` times as much, so the waveform keeps its
//!   shape and sounds the same); frames aligned that way add their fundamentals at every step of
//!   a morph. [`bake`] does it when asked, [`morph_warnings`] lists the pairs of frames that
//!   cancel their fundamental without it, and [`morph_dip_db`] measures the level dip of a morph.
//!   The alignment is plain arithmetic on the harmonics (no `atan2`), so its result is the same
//!   on every platform.
//! * **Import cleanup.** [`detect_cycle`] finds the length of one cycle in a sample,
//!   [`cut_cycles`] cuts a sample into frames of exactly one cycle each, [`drop_near_duplicates`]
//!   keeps only frames that differ and [`crossfade_edges`] makes the end of a cycle meet its
//!   start. [`source_frames`] reads back the frames of a stored table.
//!
//! Everything here runs off the audio thread (the editor and the worker that stores a table).

use super::{CYCLE_LEN, FRAMES, Harmonic, MAX_HARMONICS, Synth, from_time};
use crate::det_trig::{cos_cycles, sin_cycles};
use crate::sample_read::hermite4;
use std::sync::OnceLock;

/// Samples of one frame (one cycle).
pub const FRAME_LEN: usize = CYCLE_LEN;
/// Most key frames of a table: the synth's frame count.
pub const MAX_KEYS: usize = FRAMES;
/// Harmonics the bin editor shows.
pub const BINS: usize = 64;
/// Fundamentals quieter than this fraction of the loudest harmonic count as absent.
const FUNDAMENTAL_FLOOR: f64 = 1.0e-3;
/// A fundamental cancelled by more than this many dB at the middle of a pair is warned about.
pub const CANCEL_WARN_DB: f32 = 3.0;
/// Frames closer than this (RMS of the difference over the louder RMS) are near-duplicates.
pub const DUPLICATE_TOLERANCE: f32 = 0.05;
/// Frames quieter than this RMS are silent.
const SILENT_RMS: f32 = 1.0e-5;
/// The longest cycle [`detect_cycle`] looks for, in samples.
pub const MAX_CYCLE: usize = 4096;
/// The shortest cycle [`detect_cycle`] looks for, in samples.
pub const MIN_CYCLE: usize = 16;
/// Samples of the middle of a sample that [`detect_cycle`] reads.
const DETECT_WINDOW: usize = 8192;
/// A cycle needs at least this normalised correlation to be reported.
const DETECT_FLOOR: f64 = 0.5;

/// One cycle of a table, [`FRAME_LEN`] samples.
pub type Frame = Vec<f32>;

/// One harmonic: the amplitude of its sine and its phase in cycles (see the module docs).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bin {
    /// Amplitude of the harmonic (a sine of this peak).
    pub level: f32,
    /// Phase in cycles of the harmonic, `-0.5..=0.5`.
    pub phase: f32,
}

// ------------------------------------------------------------------------------------ tables

/// `sin` and `cos` of every point of one cycle, from the deterministic sine.
fn trig() -> &'static (Vec<f64>, Vec<f64>) {
    static TABLE: OnceLock<(Vec<f64>, Vec<f64>)> = OnceLock::new();
    TABLE.get_or_init(|| {
        let turn = |n: usize| n as f64 / FRAME_LEN as f64;
        (
            (0..FRAME_LEN).map(|n| sin_cycles(turn(n))).collect(),
            (0..FRAME_LEN).map(|n| cos_cycles(turn(n))).collect(),
        )
    })
}

fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// A frame of exactly [`FRAME_LEN`] samples (shorter ones are padded with silence, longer ones
/// cut), non-finite samples read as 0.
fn fitted(frame: &[f32]) -> Frame {
    let mut out: Frame = frame.iter().take(FRAME_LEN).map(|&x| clean(x)).collect();
    out.resize(FRAME_LEN, 0.0);
    out
}

// -------------------------------------------------------------------------------- harmonics

/// The sine and cosine amplitudes of harmonic `h` (`1..FRAME_LEN / 2`) of `frame`.
fn harmonic_of(frame: &[f32], h: usize) -> (f64, f64) {
    let (sin, cos) = trig();
    let (mut s, mut c) = (0.0f64, 0.0f64);
    for (n, &x) in frame.iter().enumerate().take(FRAME_LEN) {
        let at = (h * n) & (FRAME_LEN - 1);
        s += f64::from(x) * sin[at];
        c += f64::from(x) * cos[at];
    }
    let scale = 2.0 / FRAME_LEN as f64;
    (s * scale, c * scale)
}

/// Adds `s sin + c cos` of harmonic `h` to `frame`.
fn add_harmonic(frame: &mut [f32], h: usize, s: f64, c: f64) {
    let (sin, cos) = trig();
    for (n, x) in frame.iter_mut().enumerate().take(FRAME_LEN) {
        let at = (h * n) & (FRAME_LEN - 1);
        *x += (s * sin[at] + c * cos[at]) as f32;
    }
}

fn bin_of(s: f64, c: f64) -> Bin {
    let level = (s * s + c * c).sqrt();
    let phase = if level > 1.0e-12 {
        c.atan2(s) / std::f64::consts::TAU
    } else {
        0.0
    };
    Bin {
        level: level as f32,
        phase: phase as f32,
    }
}

fn amplitudes(bin: Bin) -> (f64, f64) {
    let level = f64::from(bin.level.max(0.0));
    let phase = f64::from(bin.phase);
    (level * cos_cycles(phase), level * sin_cycles(phase))
}

/// The first `count` harmonics of `frame` (at most [`MAX_HARMONICS`]), harmonic 1 first.
pub fn analyse(frame: &[f32], count: usize) -> Vec<Bin> {
    let frame = fitted(frame);
    (1..=count.min(MAX_HARMONICS))
        .map(|h| {
            let (s, c) = harmonic_of(&frame, h);
            bin_of(s, c)
        })
        .collect()
}

/// A frame built from `bins` by additive synthesis: harmonic `n + 1` is `bins[n]`. Harmonics
/// beyond [`MAX_HARMONICS`] are ignored.
pub fn rebuild(bins: &[Bin]) -> Frame {
    let mut out = vec![0.0f32; FRAME_LEN];
    for (n, bin) in bins.iter().enumerate().take(MAX_HARMONICS) {
        if bin.level <= 0.0 {
            continue;
        }
        let (s, c) = amplitudes(*bin);
        add_harmonic(&mut out, n + 1, s, c);
    }
    out
}

/// Replaces harmonic `h` (`1..=`[`MAX_HARMONICS`]) of `frame` by `bin`; every other harmonic
/// stays as it was.
pub fn set_harmonic(frame: &mut Frame, h: usize, bin: Bin) {
    if h == 0 || h > MAX_HARMONICS {
        return;
    }
    *frame = fitted(frame);
    let (os, oc) = harmonic_of(frame, h);
    let (ns, nc) = amplitudes(bin);
    add_harmonic(frame, h, ns - os, nc - oc);
}

/// The fundamental of `frame` as `(level, phase in cycles)`.
pub fn fundamental(frame: &[f32]) -> (f32, f32) {
    let (s, c) = harmonic_of(&fitted(frame), 1);
    let bin = bin_of(s, c);
    (bin.level, bin.phase)
}

// --------------------------------------------------------------------------------- the draw

/// Sets the samples between `from` and `to` (each `(position, value)`, position in samples,
/// value `-1..=1`) to the straight line between them: one step of a freehand stroke.
pub fn paint_line(frame: &mut Frame, from: (f32, f32), to: (f32, f32)) {
    *frame = fitted(frame);
    let last = (FRAME_LEN - 1) as f32;
    let clamp = |p: (f32, f32)| (p.0.clamp(0.0, last), p.1.clamp(-1.0, 1.0));
    let (mut a, mut b) = (clamp(from), clamp(to));
    if a.0 > b.0 {
        std::mem::swap(&mut a, &mut b);
    }
    let (first, end) = (a.0.round() as usize, b.0.round() as usize);
    for (i, x) in frame.iter_mut().enumerate().take(end + 1).skip(first) {
        let t = if b.0 > a.0 {
            ((i as f32 - a.0) / (b.0 - a.0)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        *x = a.1 + (b.1 - a.1) * t;
    }
}

/// Smooths the cycle once with a circular binomial filter (`1 2 1`).
pub fn smooth(frame: &mut Frame) {
    let src = fitted(frame);
    for (i, x) in frame.iter_mut().enumerate() {
        let (p, n) = (
            src[(i + FRAME_LEN - 1) % FRAME_LEN],
            src[(i + 1) % FRAME_LEN],
        );
        *x = 0.25 * p + 0.5 * src[i] + 0.25 * n;
    }
}

/// Scales the frame so it peaks at 1 (a silent frame stays silent).
pub fn normalise(frame: &mut Frame) {
    *frame = fitted(frame);
    let peak = frame.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if peak > 1.0e-9 {
        for x in frame.iter_mut() {
            *x /= peak;
        }
    }
}

/// Takes the mean out of the frame.
pub fn remove_dc(frame: &mut Frame) {
    *frame = fitted(frame);
    let mean = frame.iter().map(|&x| f64::from(x)).sum::<f64>() / FRAME_LEN as f64;
    for x in frame.iter_mut() {
        *x -= mean as f32;
    }
}

/// Flips the polarity.
pub fn invert(frame: &mut Frame) {
    *frame = fitted(frame);
    for x in frame.iter_mut() {
        *x = -*x;
    }
}

/// Plays the cycle backwards.
pub fn reverse(frame: &mut Frame) {
    *frame = fitted(frame);
    frame.reverse();
}

/// The RMS of a frame.
pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame.iter().map(|&x| f64::from(clean(x)).powi(2)).sum();
    (sum / frame.len() as f64).sqrt() as f32
}

// ------------------------------------------------------------------------------ phase-safe

/// Shifts the frame in time until its fundamental has phase 0, keeping its shape: harmonic `h`
/// turns by `h` times the fundamental's phase. Harmonics above [`MAX_HARMONICS`], the mean and
/// the top bin are dropped (the synth does not play them either). A frame without a fundamental
/// (quieter than a thousandth of its loudest harmonic) comes back unchanged.
pub fn align_fundamental(frame: &[f32]) -> Frame {
    let frame = fitted(frame);
    let x: Vec<f64> = frame.iter().map(|&v| f64::from(v)).collect();
    let mut spec = from_time(&x);
    let loudest = spec
        .iter()
        .skip(1)
        .map(|h| h.re.hypot(h.im))
        .fold(0.0f64, f64::max);
    let first = spec[1];
    let level = first.re.hypot(first.im);
    if level <= loudest * FUNDAMENTAL_FLOOR || level < 1.0e-12 {
        return frame;
    }
    // The fundamental is `level e^{i phi}`; harmonic `h` is multiplied by `e^{-i h phi}`, the
    // h-th power of the conjugate of the unit vector.
    let (ur, ui) = (first.re / level, -first.im / level);
    let (mut pr, mut pi) = (1.0f64, 0.0f64);
    for h in spec.iter_mut().skip(1) {
        let (nr, ni) = (pr * ur - pi * ui, pr * ui + pi * ur);
        (pr, pi) = (nr, ni);
        let (re, im) = (h.re, h.im);
        *h = Harmonic {
            re: re * pr - im * pi,
            im: re * pi + im * pr,
        };
    }
    let mut out = vec![0.0f32; FRAME_LEN];
    Synth::new().render(&spec, MAX_HARMONICS, 1.0, &mut out);
    out
}

/// What is wrong with a morph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Warning {
    /// The fundamental of two neighbouring key frames cancels at the middle of the blend by
    /// this many dB (more than [`CANCEL_WARN_DB`]); phase-safe morph removes it.
    Cancels {
        /// How far the fundamental falls below the mean of the two ends, dB (positive).
        dip_db: f32,
    },
    /// The key frame has no fundamental, so phase-safe morph cannot align it.
    NoFundamental,
}

/// A warning about a key frame: for [`Warning::Cancels`] the blend between key frame `frame` and
/// the next one, for [`Warning::NoFundamental`] the frame itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MorphWarning {
    /// Index of the key frame.
    pub frame: usize,
    /// The problem.
    pub kind: Warning,
}

/// The warnings about `keys`. Without `phase_safe`, every neighbouring pair whose fundamentals
/// cancel; with it, the frames that cannot be aligned.
pub fn morph_warnings(keys: &[Frame], phase_safe: bool) -> Vec<MorphWarning> {
    let first: Vec<(f64, f64)> = keys.iter().map(|k| harmonic_of(&fitted(k), 1)).collect();
    let mut out = Vec::new();
    if phase_safe {
        for (n, key) in keys.iter().enumerate() {
            let spec = analyse(key, BINS);
            let loudest = spec.iter().fold(0.0f32, |m, b| m.max(b.level));
            let level = spec.first().map_or(0.0, |b| b.level);
            if loudest > 1.0e-6 && f64::from(level) <= f64::from(loudest) * FUNDAMENTAL_FLOOR {
                out.push(MorphWarning {
                    frame: n,
                    kind: Warning::NoFundamental,
                });
            }
        }
        return out;
    }
    for (n, pair) in first.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        let (la, lb) = (a.0.hypot(a.1), b.0.hypot(b.1));
        let mean = 0.5 * (la + lb);
        if mean < 1.0e-9 {
            continue;
        }
        let mid = 0.5 * (a.0 + b.0).hypot(a.1 + b.1);
        let dip = -20.0 * (mid.max(1.0e-9) / mean).log10();
        if dip > f64::from(CANCEL_WARN_DB) {
            out.push(MorphWarning {
                frame: n,
                kind: Warning::Cancels { dip_db: dip as f32 },
            });
        }
    }
    out
}

// ------------------------------------------------------------------------------------- bake

/// The table frame (`0..`[`FRAMES`]) that key frame `key` of `count` sits at.
pub fn key_position(key: usize, count: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    let last = (FRAMES - 1) as f64;
    ((key as f64 * last / (count - 1) as f64).round() as usize).min(FRAMES - 1)
}

/// The key frames as the table the synth stores: [`FRAMES`] frames of [`FRAME_LEN`] samples in
/// one list. Key frame `i` of `n` sits at table frame [`key_position`]; the frames between two
/// keys are their linear blend; with one key frame every table frame is that frame, with none
/// the table is silent. `phase_safe` aligns every key frame first ([`align_fundamental`]).
/// Samples are not scaled: the synth peak-normalises every frame when it builds its table.
pub fn bake(keys: &[Frame], phase_safe: bool) -> Vec<f32> {
    let keys: Vec<Frame> = keys
        .iter()
        .take(MAX_KEYS)
        .map(|k| {
            if phase_safe {
                align_fundamental(k)
            } else {
                fitted(k)
            }
        })
        .collect();
    let mut out = vec![0.0f32; FRAMES * FRAME_LEN];
    if keys.is_empty() {
        return out;
    }
    let at: Vec<usize> = (0..keys.len())
        .map(|i| key_position(i, keys.len()))
        .collect();
    for f in 0..FRAMES {
        let dst = &mut out[f * FRAME_LEN..(f + 1) * FRAME_LEN];
        // The key at or before this frame and the one after it.
        let below = at.iter().rposition(|&p| p <= f).unwrap_or(0);
        let above = (below + 1).min(keys.len() - 1);
        let span = at[above].saturating_sub(at[below]);
        let t = if span == 0 {
            0.0
        } else {
            (f - at[below]) as f32 / span as f32
        };
        for (i, d) in dst.iter_mut().enumerate() {
            let (a, b) = (keys[below][i], keys[above][i]);
            *d = a + (b - a) * t;
        }
    }
    out
}

/// The most the RMS of the blend of `a` and `b` falls below the straight line between their own
/// RMS values over `steps` points of the morph, in dB (0 for no dip, positive for a dip).
/// `phase_safe` aligns both frames first. A pair with a silent end measures 0.
pub fn morph_dip_db(a: &[f32], b: &[f32], phase_safe: bool, steps: usize) -> f32 {
    let (a, b) = if phase_safe {
        (align_fundamental(a), align_fundamental(b))
    } else {
        (fitted(a), fitted(b))
    };
    let (ra, rb) = (rms(&a), rms(&b));
    if ra < SILENT_RMS || rb < SILENT_RMS {
        return 0.0;
    }
    let steps = steps.max(2);
    let mut worst = 0.0f32;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let blend: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x + (y - x) * t).collect();
        let want = ra + (rb - ra) * t;
        let got = rms(&blend).max(1.0e-9);
        worst = worst.max(20.0 * (want / got).log10());
    }
    worst
}

// ----------------------------------------------------------------------------- stored table

/// The key frames of a stored table's samples (`source` as the synth cuts it): the frames of
/// [`FRAME_LEN`] samples that the table is built from, the repeats of one frame that a short
/// source is filled up with merged into one. A source of less than one frame is one cycle of
/// any length, stretched. Non-finite samples read as 0. Empty for a source shorter than two
/// samples.
pub fn source_frames(source: &[f32]) -> Vec<Frame> {
    if source.len() < 2 {
        return Vec::new();
    }
    let whole = source.len() / FRAME_LEN;
    if whole == 0 {
        return vec![stretch(source)];
    }
    let mut picks: Vec<usize> = Vec::new();
    for frame in 0..FRAMES {
        let pick = if whole <= 1 {
            0
        } else {
            (frame * (whole - 1) + (FRAMES - 1) / 2) / (FRAMES - 1)
        };
        if picks.last() != Some(&pick) {
            picks.push(pick);
        }
    }
    picks
        .into_iter()
        .map(|p| fitted(&source[p * FRAME_LEN..(p + 1) * FRAME_LEN]))
        .collect()
}

/// One cycle of any length stretched over a frame (linear, wrapping).
fn stretch(source: &[f32]) -> Frame {
    let n = source.len();
    (0..FRAME_LEN)
        .map(|k| {
            let pos = k as f64 * n as f64 / FRAME_LEN as f64;
            let i = (pos as usize).min(n - 1);
            let f = (pos - i as f64) as f32;
            let (a, b) = (clean(source[i]), clean(source[(i + 1) % n]));
            a + (b - a) * f
        })
        .collect()
}

// ---------------------------------------------------------------------------- import cleanup

/// The length of one cycle of a sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cycle {
    /// Samples in one cycle (fractional).
    pub length: f32,
    /// How well the sample repeats at that length, `0..=1`.
    pub strength: f32,
}

impl Cycle {
    /// The pitch of the cycle in Hz at `sample_rate`.
    pub fn hz(&self, sample_rate: f32) -> f32 {
        sample_rate / self.length.max(1.0)
    }
}

/// Finds the length of one cycle in `source` (mono): the first strong peak of the normalised
/// autocorrelation of the middle of the sample, between [`MIN_CYCLE`] and [`MAX_CYCLE`]
/// samples, refined to a fraction of a sample. `None` for a sample that is too short, silent or
/// does not repeat (noise).
pub fn detect_cycle(source: &[f32]) -> Option<Cycle> {
    let window = source.len().min(DETECT_WINDOW);
    if window < 2 * MIN_CYCLE + 2 {
        return None;
    }
    let start = (source.len() - window) / 2;
    let x: Vec<f64> = source[start..start + window]
        .iter()
        .map(|&v| f64::from(clean(v)))
        .collect();
    let max_lag = MAX_CYCLE.min(window / 2);
    if max_lag <= MIN_CYCLE + 1 {
        return None;
    }
    // r(lag) = 2 sum x[i] x[i+lag] / sum (x[i]^2 + x[i+lag]^2), over the overlap.
    let r: Vec<f64> = (0..=max_lag)
        .map(|lag| {
            let (mut num, mut den) = (0.0f64, 0.0f64);
            for i in 0..window - lag {
                num += x[i] * x[i + lag];
                den += x[i] * x[i] + x[i + lag] * x[i + lag];
            }
            if den > 1.0e-12 { 2.0 * num / den } else { 0.0 }
        })
        .collect();
    // Key maxima: the highest point of each stretch where the curve is above zero.
    let mut keys: Vec<usize> = Vec::new();
    let mut lag = 1;
    while lag < max_lag {
        if r[lag] > 0.0 && r[lag - 1] <= 0.0 {
            let mut best = lag;
            while lag < max_lag && r[lag] > 0.0 {
                if r[lag] > r[best] {
                    best = lag;
                }
                lag += 1;
            }
            if best >= MIN_CYCLE && best < max_lag {
                keys.push(best);
            }
        } else {
            lag += 1;
        }
    }
    let top = keys.iter().map(|&k| r[k]).fold(0.0f64, f64::max);
    if top < DETECT_FLOOR {
        return None;
    }
    let pick = *keys.iter().find(|&&k| r[k] >= 0.9 * top)?;
    // Parabolic refinement around the peak.
    let (a, b, c) = (r[pick - 1], r[pick], r[pick + 1]);
    let bend = a - 2.0 * b + c;
    let shift = if bend.abs() > 1.0e-12 {
        (0.5 * (a - c) / bend).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    Some(Cycle {
        length: (pick as f64 + shift) as f32,
        strength: b.clamp(0.0, 1.0) as f32,
    })
}

/// Cuts `source` (mono) into frames of exactly one cycle of `cycle` samples each (consecutive,
/// starting at the first sample), every one stretched to [`FRAME_LEN`] samples with a four-point
/// Hermite read. At most `max_frames` frames: more cycles give evenly spaced ones. A source
/// shorter than one cycle is one stretched frame.
pub fn cut_cycles(source: &[f32], cycle: f32, max_frames: usize) -> Vec<Frame> {
    let max_frames = max_frames.clamp(1, MAX_KEYS);
    if source.len() < 2 || cycle.is_nan() || cycle < 2.0 {
        return Vec::new();
    }
    let whole = (source.len() as f64 / f64::from(cycle)).floor() as usize;
    if whole == 0 {
        return vec![stretch(source)];
    }
    let take = whole.min(max_frames);
    let read = |pos: f64| -> f32 {
        let last = source.len() as i64 - 1;
        let i = pos.floor() as i64;
        let f = (pos - i as f64) as f32;
        let at = |k: i64| clean(source[k.clamp(0, last) as usize]);
        hermite4(at(i - 1), at(i), at(i + 1), at(i + 2), f)
    };
    (0..take)
        .map(|n| {
            let j = if take == 1 {
                0
            } else {
                (n * (whole - 1) + (take - 1) / 2) / (take - 1)
            };
            let start = j as f64 * f64::from(cycle);
            (0..FRAME_LEN)
                .map(|k| read(start + k as f64 * f64::from(cycle) / FRAME_LEN as f64))
                .collect()
        })
        .collect()
}

/// Keeps the frames that differ: a frame is dropped when it is silent or when it is within
/// `tolerance` of the last frame kept (RMS of the difference over the louder RMS, after both
/// have their fundamental aligned, so a drift of the cycle's phase does not make frames look
/// different). `tolerance` 0 drops only exact repeats.
pub fn drop_near_duplicates(frames: &[Frame], tolerance: f32) -> Vec<Frame> {
    let mut kept: Vec<(Frame, Frame)> = Vec::new();
    for frame in frames {
        let frame = fitted(frame);
        if rms(&frame) < SILENT_RMS {
            continue;
        }
        let aligned = align_fundamental(&frame);
        if let Some((_, last)) = kept.last() {
            let diff: Vec<f32> = last.iter().zip(&aligned).map(|(a, b)| a - b).collect();
            let louder = rms(last).max(rms(&aligned));
            if rms(&diff) <= tolerance * louder {
                continue;
            }
        }
        kept.push((frame, aligned));
    }
    kept.into_iter().map(|(f, _)| f).collect()
}

/// Makes the end of the cycle meet its start. The last `overlap` samples are blended into the
/// start (`overlap` samples are taken off the cycle, the first `overlap` samples fade in from
/// the ones that would have come before them), and the shorter cycle is stretched back to
/// [`FRAME_LEN`] samples. An `overlap` of 0 changes nothing; at most half the frame is used.
pub fn crossfade_edges(frame: &[f32], overlap: usize) -> Frame {
    let frame = fitted(frame);
    let overlap = overlap.min(FRAME_LEN / 2);
    if overlap == 0 {
        return frame;
    }
    let len = FRAME_LEN - overlap;
    let mut cycle = frame[..len].to_vec();
    for (i, c) in cycle.iter_mut().enumerate().take(overlap) {
        let t = i as f32 / overlap as f32;
        *c = frame[i] * t + frame[len + i] * (1.0 - t);
    }
    (0..FRAME_LEN)
        .map(|k| {
            let pos = k as f64 * len as f64 / FRAME_LEN as f64;
            let i = pos.floor() as usize;
            let f = (pos - i as f64) as f32;
            let at = |n: usize| cycle[n % len];
            hermite4(at(i + len - 1), at(i), at(i + 1), at(i + 2), f)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wavetable::build_pyramid;

    fn bins(f: impl Fn(usize) -> f32) -> Vec<Bin> {
        (1..=BINS)
            .map(|h| Bin {
                level: f(h),
                phase: 0.0,
            })
            .collect()
    }

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    /// An ideal sample of `f` at point `n` of a cycle.
    fn at(n: usize) -> f64 {
        n as f64 / FRAME_LEN as f64
    }

    #[test]
    fn one_harmonic_rebuilds_as_a_sine() {
        let mut b = bins(|_| 0.0);
        b[0].level = 0.8;
        let frame = rebuild(&b);
        for (n, &x) in frame.iter().enumerate() {
            let want = 0.8 * (std::f64::consts::TAU * at(n)).sin();
            assert!(close(x, want as f32, 1.0e-5), "sample {n}: {x} vs {want}");
        }
        let mut b3 = bins(|_| 0.0);
        b3[2].level = 0.5;
        b3[2].phase = 0.25;
        let frame = rebuild(&b3);
        for (n, &x) in frame.iter().enumerate() {
            let want = 0.5 * (std::f64::consts::TAU * (3.0 * at(n) + 0.25)).sin();
            assert!(close(x, want as f32, 1.0e-5), "sample {n}");
        }
    }

    #[test]
    fn a_saw_from_one_over_h_is_the_band_limited_ramp() {
        // sum sin(h x) / h = (pi - x) / 2 on (0, 2 pi): a falling ramp, overshooting at the jump.
        let b = bins(|h| 2.0 / (std::f32::consts::PI * h as f32));
        let frame = rebuild(&b);
        let mut worst = 0.0f64;
        // Away from the jump at the cycle ends the 64-harmonic sum sits close to the ramp.
        for (n, &x) in frame
            .iter()
            .enumerate()
            .take(FRAME_LEN * 9 / 10)
            .skip(FRAME_LEN / 10)
        {
            let ideal = 1.0 - 2.0 * at(n);
            worst = worst.max((f64::from(x) - ideal).abs());
        }
        assert!(worst < 0.05, "worst error {worst}");
        assert!(frame[0].abs() < 1.0e-4, "starts at zero: {}", frame[0]);
        assert!(frame[FRAME_LEN / 4] > 0.4 && frame[3 * FRAME_LEN / 4] < -0.4);
    }

    #[test]
    fn a_square_from_odd_harmonics_is_flat_between_its_edges() {
        let b = bins(|h| {
            if h % 2 == 1 {
                4.0 / (std::f32::consts::PI * h as f32)
            } else {
                0.0
            }
        });
        let frame = rebuild(&b);
        let mut worst = 0.0f32;
        for n in FRAME_LEN / 8..3 * FRAME_LEN / 8 {
            worst = worst.max((frame[n] - 1.0).abs());
            worst = worst.max((frame[n + FRAME_LEN / 2] + 1.0).abs());
        }
        assert!(worst < 0.06, "flat tops within {worst}");
    }

    #[test]
    fn analysis_returns_what_rebuild_was_given() {
        let given: Vec<Bin> = (1..=BINS)
            .map(|h| Bin {
                level: 0.5 / h as f32,
                phase: ((h * 7) % 10) as f32 / 10.0 - 0.45,
            })
            .collect();
        let back = analyse(&rebuild(&given), BINS);
        for (h, (a, b)) in given.iter().zip(&back).enumerate() {
            assert!(close(a.level, b.level, 1.0e-4), "level {h}");
            assert!(close(a.phase, b.phase, 2.0e-3), "phase {h}: {a:?} {b:?}");
        }
    }

    #[test]
    fn setting_one_harmonic_leaves_the_others_alone() {
        let mut frame = rebuild(&bins(|h| 0.3 / h as f32));
        // A harmonic above the bins stays too.
        set_harmonic(
            &mut frame,
            100,
            Bin {
                level: 0.1,
                phase: 0.0,
            },
        );
        let before = analyse(&frame, 120);
        set_harmonic(
            &mut frame,
            5,
            Bin {
                level: 0.6,
                phase: 0.25,
            },
        );
        let after = analyse(&frame, 120);
        for h in 0..120 {
            if h == 4 {
                assert!(close(after[h].level, 0.6, 1.0e-4));
                assert!(close(after[h].phase, 0.25, 2.0e-3));
            } else {
                assert!(
                    close(before[h].level, after[h].level, 1.0e-4),
                    "harmonic {}",
                    h + 1
                );
            }
        }
    }

    #[test]
    fn a_stroke_sets_the_samples_it_crosses() {
        let mut frame = vec![0.0f32; FRAME_LEN];
        paint_line(&mut frame, (100.0, 0.0), (200.0, 1.0));
        assert!(close(frame[100], 0.0, 1.0e-6));
        assert!(close(frame[150], 0.5, 1.0e-2));
        assert!(close(frame[200], 1.0, 1.0e-6));
        assert!(close(frame[99], 0.0, 1.0e-6) && close(frame[201], 0.0, 1.0e-6));
        // Backwards strokes and out-of-range ones are the same line, clamped.
        let mut other = vec![0.0f32; FRAME_LEN];
        paint_line(&mut other, (200.0, 1.0), (100.0, 0.0));
        assert_eq!(frame, other);
        paint_line(&mut other, (-50.0, 3.0), (10.0, 3.0));
        assert!(other[0..=10].iter().all(|&x| close(x, 1.0, 1.0e-6)));
    }

    #[test]
    fn processes_do_what_they_say() {
        let mut f = rebuild(&bins(|h| if h == 2 { 0.25 } else { 0.0 }));
        for x in &mut f {
            *x += 0.1;
        }
        remove_dc(&mut f);
        assert!(f.iter().map(|&x| f64::from(x)).sum::<f64>().abs() < 1.0e-3);
        normalise(&mut f);
        assert!(close(
            f.iter().fold(0.0f32, |m, x| m.max(x.abs())),
            1.0,
            1.0e-6
        ));
        let g = f.clone();
        invert(&mut f);
        assert!(close(f[10], -g[10], 1.0e-9));
        reverse(&mut f);
        assert!(close(f[0], -g[FRAME_LEN - 1], 1.0e-9));
        let mut silent = vec![0.0f32; FRAME_LEN];
        normalise(&mut silent);
        assert!(silent.iter().all(|&x| x == 0.0));
        let mut spike = vec![0.0f32; FRAME_LEN];
        spike[7] = 1.0;
        smooth(&mut spike);
        assert!(close(spike[7], 0.5, 1.0e-6) && close(spike[6], 0.25, 1.0e-6));
    }

    fn saw() -> Frame {
        rebuild(&bins(|h| 1.0 / h as f32))
    }

    /// `frame` delayed by `turn` cycles, through its harmonics.
    fn shifted(frame: &[f32], turn: f32) -> Frame {
        let b: Vec<Bin> = analyse(frame, 200)
            .into_iter()
            .enumerate()
            .map(|(n, b)| Bin {
                level: b.level,
                phase: b.phase - (n + 1) as f32 * turn,
            })
            .collect();
        // Wrap the phases into range.
        let b: Vec<Bin> = b
            .into_iter()
            .map(|b| Bin {
                level: b.level,
                phase: b.phase - b.phase.round(),
            })
            .collect();
        rebuild(&b)
    }

    #[test]
    fn aligning_zeroes_the_fundamental_phase_and_keeps_the_shape() {
        for turn in [0.0f32, 0.13, 0.5, 0.77] {
            let frame = shifted(&saw(), turn);
            let aligned = align_fundamental(&frame);
            let (level, phase) = fundamental(&aligned);
            assert!(level > 0.5, "fundamental kept: {level}");
            assert!(phase.abs() < 2.0e-3, "turn {turn}: phase {phase}");
            // The same waveform up to a shift: every level is unchanged.
            let a = analyse(&frame, 40);
            let b = analyse(&aligned, 40);
            for h in 0..40 {
                assert!(close(a[h].level, b[h].level, 2.0e-3), "harmonic {}", h + 1);
            }
            assert!(close(rms(&frame), rms(&aligned), 1.0e-3));
        }
        // Frames that are the same up to a shift align to the same samples.
        let one = align_fundamental(&shifted(&saw(), 0.21));
        let two = align_fundamental(&shifted(&saw(), 0.64));
        for (a, b) in one.iter().zip(&two) {
            assert!(close(*a, *b, 5.0e-3));
        }
    }

    #[test]
    fn a_frame_with_no_fundamental_is_left_alone() {
        let mut b = bins(|_| 0.0);
        b[1].level = 0.5;
        b[2].level = 0.4;
        let frame = rebuild(&b);
        assert_eq!(align_fundamental(&frame), frame);
        let w = morph_warnings(&[frame, saw()], true);
        assert_eq!(
            w,
            vec![MorphWarning {
                frame: 0,
                kind: Warning::NoFundamental
            }]
        );
    }

    #[test]
    fn the_phase_safe_morph_keeps_the_level_where_a_plain_one_dips() {
        let a = saw();
        // A half cycle later every odd harmonic is flipped: the fundamental cancels half way.
        let half = shifted(&a, 0.5);
        let plain = morph_dip_db(&a, &half, false, 16);
        let safe = morph_dip_db(&a, &half, true, 16);
        assert!(plain > 4.0, "the plain morph dips {plain} dB");
        assert!(safe < 1.0, "the phase-safe morph dips {safe} dB");
        // An arbitrary offset, and a table of several frames.
        let other = shifted(&a, 0.37);
        assert!(morph_dip_db(&a, &other, false, 16) > 1.5);
        assert!(morph_dip_db(&a, &other, true, 16) < 1.0);
        let mut b = bins(|h| if h % 2 == 1 { 1.0 / h as f32 } else { 0.0 });
        b[0].phase = 0.31;
        let odd = rebuild(&b);
        assert!(morph_dip_db(&a, &odd, true, 16) < 1.0);
    }

    #[test]
    fn warnings_name_the_pairs_whose_fundamental_cancels() {
        let a = saw();
        let flipped = shifted(&a, 0.5);
        let keys = vec![a.clone(), a.clone(), flipped];
        let w = morph_warnings(&keys, false);
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].frame, 1);
        match w[0].kind {
            Warning::Cancels { dip_db } => assert!(dip_db > 20.0, "{dip_db}"),
            other => panic!("{other:?}"),
        }
        assert!(morph_warnings(&keys, true).is_empty());
        assert!(morph_warnings(&[a.clone(), a], false).is_empty());
    }

    #[test]
    fn bake_puts_the_keys_in_place_and_blends_between() {
        let a = saw();
        let mut sine_bins = bins(|_| 0.0);
        sine_bins[0].level = 1.0;
        let sine = rebuild(&sine_bins);
        let table = bake(&[a.clone(), sine.clone()], false);
        assert_eq!(table.len(), FRAMES * FRAME_LEN);
        assert_eq!(&table[..FRAME_LEN], &a[..]);
        assert_eq!(&table[(FRAMES - 1) * FRAME_LEN..], &sine[..]);
        // Frame 3 of 15 is a fifth of the way.
        let want = a[100] + (sine[100] - a[100]) * (3.0 / 15.0);
        assert!(close(table[3 * FRAME_LEN + 100], want, 1.0e-6));
        // Four keys sit at 0, 5, 10 and 15.
        let keys: Vec<Frame> = (0..4)
            .map(|i| {
                let mut b = bins(|_| 0.0);
                b[i].level = 1.0;
                rebuild(&b)
            })
            .collect();
        let table = bake(&keys, false);
        for (i, key) in keys.iter().enumerate() {
            let p = key_position(i, 4);
            assert_eq!(p, [0, 5, 10, 15][i]);
            assert_eq!(&table[p * FRAME_LEN..(p + 1) * FRAME_LEN], &key[..]);
        }
        // One key fills the table, none silences it.
        let one = bake(std::slice::from_ref(&a), false);
        assert!(one.chunks(FRAME_LEN).all(|c| c == &a[..]));
        assert!(bake(&[], false).iter().all(|&x| x == 0.0));
        // Sixteen keys are the table itself.
        let sixteen: Vec<Frame> = (0..16).map(|i| shifted(&a, i as f32 / 40.0)).collect();
        let table = bake(&sixteen, false);
        for (i, key) in sixteen.iter().enumerate() {
            assert_eq!(&table[i * FRAME_LEN..(i + 1) * FRAME_LEN], &key[..]);
        }
    }

    #[test]
    fn bake_is_deterministic_and_phase_safe_aligns_every_key() {
        let a = saw();
        let b = shifted(&a, 0.5);
        let one = bake(&[a.clone(), b.clone()], true);
        let two = bake(&[a, b], true);
        assert_eq!(one, two);
        for f in 0..FRAMES {
            let (level, phase) = fundamental(&one[f * FRAME_LEN..(f + 1) * FRAME_LEN]);
            assert!(level > 0.4, "frame {f} keeps its fundamental ({level})");
            assert!(phase.abs() < 3.0e-3, "frame {f}: phase {phase}");
        }
    }

    #[test]
    fn a_baked_table_reads_back_as_its_key_frames() {
        let keys: Vec<Frame> = (0..4).map(|i| shifted(&saw(), 0.1 * i as f32)).collect();
        let table = bake(&keys, false);
        let frames = source_frames(&table);
        assert_eq!(frames.len(), FRAMES, "every baked frame differs");
        for (i, key) in keys.iter().enumerate() {
            assert_eq!(&frames[key_position(i, 4)], key);
        }
        // The synth builds the same table from it (a table of 16 whole frames is not resampled).
        assert!(build_pyramid(&table).is_some());
        // Short sources.
        let two = [keys[0].clone(), keys[1].clone()].concat();
        assert_eq!(source_frames(&two).len(), 2);
        let tone: Vec<f32> = (0..300)
            .map(|n| (std::f64::consts::TAU * n as f64 / 300.0).sin() as f32)
            .collect();
        assert_eq!(source_frames(&tone).len(), 1);
        assert!(source_frames(&[1.0]).is_empty());
    }

    fn tone(len: usize, cycle: f64) -> Vec<f32> {
        (0..len)
            .map(|n| {
                let p = n as f64 / cycle;
                ((std::f64::consts::TAU * p).sin() + 0.4 * (std::f64::consts::TAU * 2.0 * p).sin())
                    as f32
            })
            .collect()
    }

    #[test]
    fn the_cycle_of_a_tone_is_found() {
        for cycle in [100.0f64, 441.0, 1000.5, 2048.0] {
            let found = detect_cycle(&tone(20_000, cycle)).expect("a tone has a cycle");
            let err = (f64::from(found.length) - cycle).abs() / cycle;
            assert!(err < 0.005, "cycle {cycle}: found {}", found.length);
            assert!(found.strength > 0.9);
        }
        let found = detect_cycle(&tone(20_000, 441.0)).unwrap();
        assert!(close(found.hz(44_100.0), 100.0, 0.5));
    }

    #[test]
    fn noise_and_silence_have_no_cycle() {
        assert!(detect_cycle(&vec![0.0; 10_000]).is_none());
        assert!(detect_cycle(&[0.5; 10]).is_none());
        let mut rng = crate::Rng::new(5);
        let noise: Vec<f32> = (0..10_000).map(|_| rng.next_f32() * 2.0 - 1.0).collect();
        assert!(detect_cycle(&noise).is_none());
    }

    #[test]
    fn cutting_by_cycle_gives_frames_of_one_cycle() {
        let source = tone(441 * 6, 441.0);
        let frames = cut_cycles(&source, 441.0, 16);
        assert_eq!(frames.len(), 6);
        // Each frame is one sine plus its octave over the whole frame.
        for f in &frames {
            for n in (0..FRAME_LEN).step_by(97) {
                let p = n as f64 / FRAME_LEN as f64;
                let want = (std::f64::consts::TAU * p).sin()
                    + 0.4 * (std::f64::consts::TAU * 2.0 * p).sin();
                assert!(close(f[n], want as f32, 5.0e-3), "sample {n}");
            }
        }
        assert_eq!(cut_cycles(&tone(441 * 40, 441.0), 441.0, 4).len(), 4);
        assert_eq!(cut_cycles(&tone(200, 441.0), 441.0, 4).len(), 1);
        assert!(cut_cycles(&source, 0.5, 4).is_empty());
    }

    #[test]
    fn near_duplicates_and_silence_are_dropped() {
        let a = saw();
        let b = shifted(&a, 0.3);
        let mut c = bins(|h| 1.0 / (h * h) as f32);
        c[2].level = 0.5;
        let c = rebuild(&c);
        let mut near = a.clone();
        near[10] += 0.001;
        let silent = vec![0.0f32; FRAME_LEN];
        let kept = drop_near_duplicates(
            &[a.clone(), near, silent, b, c.clone(), c.clone()],
            DUPLICATE_TOLERANCE,
        );
        assert_eq!(kept.len(), 2, "the saw and the other frame stay");
        assert_eq!(kept[0], a);
        assert_eq!(kept[1], c);
        // Zero tolerance only drops exact repeats.
        let kept = drop_near_duplicates(&[a.clone(), a.clone(), c], 0.0);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn the_crossfade_closes_the_jump_at_the_cycle_end() {
        // One and a quarter turns of a sine: the end does not meet the start.
        let frame: Vec<f32> = (0..FRAME_LEN)
            .map(|n| (std::f64::consts::TAU * 1.25 * at(n)).sin() as f32)
            .collect();
        let jump = |f: &[f32]| (f[0] - f[FRAME_LEN - 1]).abs();
        let step = |f: &[f32]| {
            f.windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max)
        };
        let before = jump(&frame);
        assert!(before > 0.5, "the jump is {before}");
        let fixed = crossfade_edges(&frame, 400);
        assert_eq!(fixed.len(), FRAME_LEN);
        assert!(
            jump(&fixed) <= step(&fixed) * 1.5,
            "the end meets the start: {} against steps of {}",
            jump(&fixed),
            step(&fixed)
        );
        assert_eq!(crossfade_edges(&frame, 0), frame);
        assert_eq!(crossfade_edges(&frame, 5000).len(), FRAME_LEN);
    }

    #[test]
    fn non_finite_samples_read_as_silence() {
        let mut f = saw();
        f[3] = f32::NAN;
        f[9] = f32::INFINITY;
        assert!(align_fundamental(&f).iter().all(|x| x.is_finite()));
        assert!(analyse(&f, 8).iter().all(|b| b.level.is_finite()));
        assert!(bake(&[f], true).iter().all(|x| x.is_finite()));
        assert_eq!(rebuild(&[]).len(), FRAME_LEN);
    }
}
