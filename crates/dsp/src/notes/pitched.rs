//! Monophonic pitch to notes: a YIN tracker with a voicing decision, median smoothing and a
//! segmentation by pitch stability and amplitude re-attacks.
//!
//! **Algorithm** (lengths scale with the rate):
//! 1. *Mono and decimation:* the mean of the channels, averaged down by `D` (a box filter) so that
//!    the highest key of the range has at least six samples per period; `D` is 1 to 8.
//! 2. *Frames:* every 5 ms a window of `W + tau_max` samples is analysed, `tau_max` the period of
//!    the lowest key of the range and `W = 1.5 * tau_max`; the signal is zero padded by half a
//!    window at both ends so that frame `k` is centred on source frame `k * hop`.
//! 3. *Level gate:* a frame whose RMS is below the loudest frame by `30 + 30 * s` dB (`s` the
//!    sensitivity, and never below -70 dBFS) is unvoiced and costs nothing.
//! 4. *YIN* (de Cheveigne and Kawahara, 2002): the squared difference function, its cumulative
//!    mean normalisation (`cmnd`), the first dip below the threshold `0.10 + 0.30 * s` walked down
//!    to its minimum, and a parabolic interpolation of the period. No dip is an unvoiced frame.
//!    The pitch is `69 + 12 * log2(f / 440)` in fractional semitones.
//! 5. *Smoothing:* a median over five frames (voiced neighbours only).
//! 6. *Segmentation:* a note continues while the pitch stays within the tolerance of its running
//!    median (vibrato and glides inside it do not split); a deviation starts a candidate that
//!    becomes a new note only when it holds still for the stable time (half the minimum note
//!    length, at least 30 ms), so a blip does not. Unvoiced gaps up to 25 ms are bridged. An
//!    amplitude onset ([`crate::onset::detect_onsets`]) inside a note, at least 30 ms after its
//!    start, splits it: a re-attack of the same pitch.
//! 7. *Notes:* the key is the median pitch rounded to a semitone (kept inside the range). A note
//!    that follows silence starts at the nearest onset (else a little before its first voiced
//!    frame); one that follows another with no gap starts where the pitch moved. A note ends where
//!    the voicing ends or where its level has decayed `24 + 12 * s` dB below its peak, whichever
//!    comes first, and never after the next note starts. Notes shorter than the minimum are
//!    dropped, and two notes of one key with at most 40 ms between them and no onset at the second
//!    are joined. The velocity follows the peak level in dB, relative to the loudest note.

use super::{ExtractedNote, Levels, clamp_sensitivity, mono_mix, velocity_for};
use crate::det_math::{exp2_det, log2_det};
use crate::onset::detect_onsets;

/// Time between two frames, in milliseconds.
const HOP_MS: u64 = 5;
/// The longest unvoiced stretch inside a note, in milliseconds.
const BRIDGE_MS: u64 = 25;
/// The longest gap between two notes of one key that joins them, in milliseconds.
const MERGE_GAP_MS: u64 = 40;
/// The shortest time into a note at which a re-attack splits it, in milliseconds.
const SPLIT_AFTER_MS: u64 = 30;
/// How far before the first voiced frame an onset may be and still start the note, in ms.
const ONSET_BEFORE_MS: u64 = 70;
/// How far after the first voiced frame an onset may be and still start the note, in ms.
const ONSET_AFTER_MS: u64 = 20;
/// Peak level below which a take is silent (about -80 dBFS).
const MIN_PEAK: f32 = 1.0e-4;
/// Frame RMS below which a frame is never voiced (about -70 dBFS).
const MIN_RMS: f32 = 3.0e-4;
/// Frames of the pitch the running median of a note looks at.
const RUN_MEDIAN: usize = 24;

/// What to extract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchParams {
    /// `0..=1`: more finds quieter and less periodic sound and more re-attacks (NaN gives 0.5).
    pub sensitivity: f32,
    /// Notes shorter than this are dropped, in ms (10..=2000).
    pub min_note_ms: u32,
    /// Lowest key looked for.
    pub key_lo: u8,
    /// Highest key looked for.
    pub key_hi: u8,
    /// How far, in cents, the pitch may drift inside one note (vibrato, a glide): 20..=300.
    pub tolerance_cents: u32,
}

impl Default for PitchParams {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            min_note_ms: 60,
            key_lo: 36,
            key_hi: 96,
            tolerance_cents: 70,
        }
    }
}

/// The frequency of a (fractional) MIDI key.
fn key_hz(key: f32) -> f64 {
    440.0 * f64::from(exp2_det((key - 69.0) / 12.0))
}

/// One analysed frame.
#[derive(Debug, Clone, Copy)]
struct Frame {
    voiced: bool,
    /// Fractional semitones (MIDI key).
    pitch: f32,
}

/// How a segment began.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Start {
    /// After unvoiced frames.
    Gap,
    /// The pitch moved.
    Pitch,
    /// An amplitude re-attack at this source frame.
    Onset(u64),
}

#[derive(Debug, Clone)]
struct Seg {
    first: usize,
    last: usize,
    pitches: Vec<f32>,
    start: Start,
}

impl Seg {
    /// The median of the most recent pitches.
    fn running(&self) -> f32 {
        let from = self.pitches.len().saturating_sub(RUN_MEDIAN);
        median(&self.pitches[from..])
    }
}

fn median(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        0.5 * (s[n / 2 - 1] + s[n / 2])
    }
}

/// The sum of the squared differences of two equal slices, in a fixed order (four lanes).
fn sq_diff(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 4];
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<4>(), b.as_chunks::<4>());
    for (x, y) in ca.iter().zip(cb) {
        for i in 0..4 {
            let d = x[i] - y[i];
            lanes[i] += d * d;
        }
    }
    let mut tail = 0.0f32;
    for (x, y) in ra.iter().zip(rb) {
        let d = x - y;
        tail += d * d;
    }
    (lanes[0] + lanes[1]) + (lanes[2] + lanes[3]) + tail
}

/// The YIN tracker for one key range and rate.
struct Tracker {
    /// Decimation factor.
    d: usize,
    sr_d: f64,
    tau_min: usize,
    tau_max: usize,
    /// Window length in decimated samples.
    w: usize,
    /// Frame hop in decimated samples.
    hop: usize,
}

impl Tracker {
    fn new(sample_rate: u32, f_lo: f64, f_hi: f64) -> Self {
        let sr = f64::from(sample_rate);
        let d = ((sr / (6.0 * f_hi)).floor() as usize).clamp(1, 8);
        let sr_d = sr / d as f64;
        let tau_min = ((sr_d / f_hi).floor() as usize).max(2);
        let tau_max = ((sr_d / f_lo).ceil() as usize + 1).max(tau_min + 2);
        Self {
            d,
            sr_d,
            tau_min,
            tau_max,
            w: tau_max * 3 / 2,
            hop: ((sr_d * HOP_MS as f64 / 1000.0).round() as usize).max(1),
        }
    }

    /// Decimated samples one frame reads.
    fn span(&self) -> usize {
        self.w + self.tau_max
    }

    /// Source frames between two analysis frames.
    fn step(&self) -> u64 {
        (self.hop * self.d) as u64
    }

    /// The period in decimated samples found in `x` (`span()` samples), or `None` when there is
    /// no dip of the normalised difference below `thr`.
    fn period(&self, x: &[f32], thr: f32, cmnd: &mut Vec<f32>) -> Option<f64> {
        let a = &x[..self.w];
        cmnd.clear();
        cmnd.resize(self.tau_max + 2, 1.0);
        let mut run = 0.0f64;
        for tau in 1..=self.tau_max {
            let d = f64::from(sq_diff(a, &x[tau..tau + self.w]));
            run += d;
            cmnd[tau] = if run > 0.0 {
                (d * tau as f64 / run) as f32
            } else {
                1.0
            };
        }
        let mut tau = self.tau_min;
        while tau <= self.tau_max {
            if cmnd[tau] < thr {
                while tau < self.tau_max && cmnd[tau + 1] < cmnd[tau] {
                    tau += 1;
                }
                let mut at = tau as f64;
                if tau > 1 && tau < self.tau_max {
                    let (l, c, r) = (cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]);
                    let den = f64::from(l) - 2.0 * f64::from(c) + f64::from(r);
                    if den > 1e-12 {
                        at += (0.5 * (f64::from(l) - f64::from(r)) / den).clamp(-1.0, 1.0);
                    }
                }
                return Some(at);
            }
            tau += 1;
        }
        None
    }

    /// Analyses `mono`: one [`Frame`] per [`Tracker::step`] source frames, frame `k` centred on
    /// source frame `k * step`.
    fn track(&self, mono: &[f32], s: f32, (key_lo, key_hi): (f32, f32)) -> Vec<Frame> {
        let nd = mono.len() / self.d;
        let span = self.span();
        let pad = span / 2;
        let mut x = vec![0.0f32; nd + 2 * pad + span];
        let inv = 1.0 / self.d as f64;
        for (i, chunk) in mono.chunks_exact(self.d).enumerate() {
            let mut sum = 0.0f64;
            for v in chunk {
                sum += f64::from(*v);
            }
            x[pad + i] = (sum * inv) as f32;
        }
        let nf = nd / self.hop + 1;
        let rms: Vec<f32> = (0..nf)
            .map(|k| {
                let mut acc = 0.0f64;
                for v in &x[k * self.hop..k * self.hop + span] {
                    acc += f64::from(*v) * f64::from(*v);
                }
                (acc / span as f64).sqrt() as f32
            })
            .collect();
        let top = rms.iter().fold(0.0f32, |m, r| m.max(*r));
        let gate_db = 30.0 + 30.0 * s;
        let gate = (top * exp2_det(-gate_db / 6.020_6)).max(MIN_RMS);
        let thr = 0.10 + 0.30 * s;
        let mut cmnd = Vec::new();
        let mut frames = Vec::with_capacity(nf);
        let unvoiced = Frame {
            voiced: false,
            pitch: 0.0,
        };
        for k in 0..nf {
            if rms[k] < gate {
                frames.push(unvoiced);
                continue;
            }
            let window = &x[k * self.hop..k * self.hop + span];
            let Some(tau) = self.period(window, thr, &mut cmnd) else {
                frames.push(unvoiced);
                continue;
            };
            let hz = (self.sr_d / tau) as f32;
            let pitch = 69.0 + 12.0 * log2_det(hz / 440.0);
            frames.push(Frame {
                voiced: pitch >= key_lo - 0.5 && pitch <= key_hi + 0.5,
                pitch,
            });
        }
        // Median over five frames, voiced neighbours only.
        let raw = frames.clone();
        for k in 0..nf {
            if !raw[k].voiced {
                continue;
            }
            let lo = k.saturating_sub(2);
            let hi = (k + 2).min(nf - 1);
            let near: Vec<f32> = raw[lo..=hi]
                .iter()
                .filter(|f| f.voiced)
                .map(|f| f.pitch)
                .collect();
            if near.len() >= 3 {
                frames[k].pitch = median(&near);
            }
        }
        frames
    }
}

/// What the segmentation and the note building share.
struct Ctx {
    /// Source frames per analysis frame.
    step: u64,
    /// Source frames one analysis window covers.
    span: u64,
    /// Frames of the take.
    total: u64,
    /// Frames per millisecond is `sr / 1000`: this is `sr`.
    sr: u64,
    sensitivity: f32,
    keys: (u8, u8),
    min_note: u64,
}

impl Ctx {
    fn ms(&self, v: u64) -> u64 {
        (v * self.sr / 1000).max(1)
    }

    /// The source frame at the centre of analysis frame `k`.
    fn centre(&self, k: usize) -> u64 {
        k as u64 * self.step
    }
}

/// The notes of the monophonic audio `channels` (planar) at `sample_rate`, sorted by start and
/// not overlapping. See the module docs for the method; silence, noise and input shorter than a
/// few tens of milliseconds give none.
pub fn extract_pitched_notes(
    channels: &[&[f32]],
    sample_rate: u32,
    params: &PitchParams,
) -> Vec<ExtractedNote> {
    if sample_rate < 8000 {
        return Vec::new();
    }
    let mono = mono_mix(channels);
    let peak = mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak < MIN_PEAK {
        return Vec::new();
    }
    let s = clamp_sensitivity(params.sensitivity);
    let (mut lo, mut hi) = (params.key_lo.min(127), params.key_hi.min(127));
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }
    let f_lo = key_hz(f32::from(lo) - 0.5).max(20.0);
    let f_hi = key_hz(f32::from(hi) + 0.5).min(f64::from(sample_rate) / 6.0);
    if f_hi <= f_lo {
        return Vec::new();
    }
    let tracker = Tracker::new(sample_rate, f_lo, f_hi);
    let span = (tracker.span() * tracker.d) as u64;
    if (mono.len() as u64) < span {
        return Vec::new();
    }
    let min_ms = u64::from(params.min_note_ms.clamp(10, 2000));
    let mut ctx = Ctx {
        step: tracker.step(),
        span,
        total: mono.len() as u64,
        sr: u64::from(sample_rate),
        sensitivity: s,
        keys: (lo, hi),
        min_note: 0,
    };
    ctx.min_note = ctx.ms(min_ms);
    let frames = tracker.track(&mono, s, (f32::from(lo), f32::from(hi)));
    let onsets = detect_onsets(&[&mono], sample_rate, s);
    let tol = params.tolerance_cents.clamp(20, 300) as f32 / 100.0;
    let hop_ms = (ctx.step * 1000 / ctx.sr).max(1);
    let stable = ((min_ms / 2).max(30) / hop_ms).max(2) as usize;
    let bridge = (BRIDGE_MS / hop_ms) as usize;
    let segs = segment(&frames, &onsets, (tol, stable, bridge), &ctx);
    let levels = Levels::new(&mono, sample_rate);
    build_notes(&segs, &onsets, &levels, &ctx)
}

/// Cuts the frames into segments (module docs, step 6).
fn segment(
    frames: &[Frame],
    onsets: &[u64],
    (tol, stable, bridge): (f32, usize, usize),
    ctx: &Ctx,
) -> Vec<Seg> {
    let split_after = ctx.ms(SPLIT_AFTER_MS);
    let mut segs = Vec::new();
    let mut cur: Option<Seg> = None;
    let mut cand: Vec<usize> = Vec::new();
    let mut unvoiced = 0usize;
    for (k, f) in frames.iter().enumerate() {
        let here = ctx.centre(k);
        if !f.voiced {
            unvoiced += 1;
            if unvoiced > bridge {
                segs.extend(cur.take());
                cand.clear();
            }
            continue;
        }
        unvoiced = 0;
        let Some(c) = cur.as_mut() else {
            cur = Some(Seg {
                first: k,
                last: k,
                pitches: vec![f.pitch],
                start: Start::Gap,
            });
            cand.clear();
            continue;
        };
        // A re-attack between the previous frame and this one, late enough into the note.
        let before = here.saturating_sub(ctx.step);
        let attack = onsets
            .iter()
            .copied()
            .find(|&o| o > before && o <= here)
            .filter(|&o| o >= ctx.centre(c.first) + split_after);
        if let Some(o) = attack {
            segs.extend(cur.take());
            cur = Some(Seg {
                first: k,
                last: k,
                pitches: vec![f.pitch],
                start: Start::Onset(o),
            });
            cand.clear();
            continue;
        }
        if (f.pitch - c.running()).abs() <= tol {
            c.pitches.push(f.pitch);
            c.last = k;
            cand.clear();
            continue;
        }
        // The pitch is away from the note: a candidate for the next one.
        if !cand.is_empty() {
            let mean = cand.iter().map(|&i| frames[i].pitch).sum::<f32>() / cand.len() as f32;
            if (f.pitch - mean).abs() > tol * 0.7 {
                cand.clear();
            }
        }
        cand.push(k);
        if cand.len() >= stable {
            let first = cand[0];
            let pitches: Vec<f32> = cand.iter().map(|&i| frames[i].pitch).collect();
            if let Some(mut done) = cur.take() {
                done.last = (done.first..first)
                    .rev()
                    .find(|&i| frames[i].voiced)
                    .unwrap_or(done.first);
                segs.push(done);
            }
            cur = Some(Seg {
                first,
                last: k,
                pitches,
                start: Start::Pitch,
            });
            cand.clear();
        }
    }
    segs.extend(cur);
    segs
}

/// A note before the notes are put in order.
struct Raw {
    start: u64,
    end: u64,
    key: u8,
    /// Peak level.
    level: f32,
    /// The note began at an amplitude onset.
    attack: bool,
}

/// Turns segments into notes (module docs, step 7).
fn build_notes(segs: &[Seg], onsets: &[u64], levels: &Levels, ctx: &Ctx) -> Vec<ExtractedNote> {
    // A pitch change is seen about this long after it happens (a window straddles the change).
    let lag = ctx.span / 8;
    let decay = exp2_det(-(24.0 + 12.0 * ctx.sensitivity) / 6.020_6);
    let (lo, hi) = ctx.keys;
    let block = levels.block() as u64;
    let mut raw: Vec<Raw> = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        if seg.pitches.len() < 3 {
            continue;
        }
        let c_first = ctx.centre(seg.first);
        let (start, attack) = match seg.start {
            Start::Onset(o) => (o, true),
            Start::Pitch => (c_first.saturating_sub(lag), false),
            Start::Gap => {
                let want = c_first.saturating_sub(ctx.span / 10);
                let from = c_first.saturating_sub(ctx.ms(ONSET_BEFORE_MS));
                let to = c_first + ctx.ms(ONSET_AFTER_MS);
                match onsets
                    .iter()
                    .copied()
                    .filter(|&o| o >= from && o <= to)
                    .min_by_key(|&o| o.abs_diff(want))
                {
                    Some(o) => (o, true),
                    None => (want, false),
                }
            }
        };
        // A note that the next one follows with no gap ends where the next starts.
        let next_start = segs.get(i + 1).and_then(|n| match n.start {
            Start::Gap => None,
            Start::Onset(o) => Some(o),
            Start::Pitch => Some(ctx.centre(n.first).saturating_sub(lag)),
        });
        let voiced_end = ctx.centre(seg.last) + ctx.step / 2;
        let mut end = next_start.unwrap_or(voiced_end).min(ctx.total);
        if end <= start {
            continue;
        }
        // The peak level, and where the note has decayed too far below it.
        let (mut top, mut at_top) = (0.0f32, start);
        let mut p = start;
        while p < end {
            let l = levels.rms_at(p);
            if l > top {
                (top, at_top) = (l, p);
            }
            p += block;
        }
        if top <= 0.0 {
            continue;
        }
        let mut p = at_top;
        while p < end {
            if levels.rms_at(p) < top * decay {
                end = p.max(start + 1);
                break;
            }
            p += block;
        }
        let key = median(&seg.pitches)
            .round()
            .clamp(f32::from(lo), f32::from(hi)) as u8;
        raw.push(Raw {
            start,
            end,
            key,
            level: top,
            attack,
        });
    }
    raw.sort_by_key(|n| (n.start, n.end));
    // Join notes of one key that a short gap separates with no re-attack.
    let merge_gap = ctx.ms(MERGE_GAP_MS);
    let mut joined: Vec<Raw> = Vec::new();
    for n in raw {
        if let Some(last) = joined.last_mut()
            && last.key == n.key
            && !n.attack
            && n.start.saturating_sub(last.end) <= merge_gap
        {
            last.end = last.end.max(n.end);
            last.level = last.level.max(n.level);
            continue;
        }
        joined.push(n);
    }
    // No overlaps; drop what is too short.
    for i in 0..joined.len().saturating_sub(1) {
        let next = joined[i + 1].start;
        joined[i].end = joined[i].end.min(next);
    }
    joined.retain(|n| n.end > n.start && n.end - n.start >= ctx.min_note);
    let reference = joined.iter().fold(0.0f32, |m, n| m.max(n.level));
    joined
        .into_iter()
        .map(|n| ExtractedNote {
            start: n.start,
            length: n.end - n.start,
            key: n.key,
            velocity: velocity_for(n.level, reference),
        })
        .collect()
}
