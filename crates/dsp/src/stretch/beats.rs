//! The beats engine: transient-aware stretching for drums and other percussive material.
//!
//! The source is cut at its onsets ([`detect_onsets`]; the start of the span is an implicit
//! cut). Every onset keeps its place on the time map: the output frame where it plays is the
//! map's inverse of the onset's source frame, so the hits stay locked to the grid however the
//! tempo moves. Each piece between two onsets is built from two parts:
//!
//! * the **attack**, the first 30 ms (less if the piece is shorter), copied verbatim at the
//!   original speed, so the hit keeps its edge, with a 2 ms crossfade from the piece before it
//!   that starts just before the onset (the piece before ends there, so the hit itself is
//!   never smeared);
//! * the **body**, the rest of the piece, stretched or squeezed to fill the time left until the
//!   next onset with WSOLA (waveform-similarity overlap-add): 30 ms Hann frames at a hop of
//!   half a frame, each placed at the source offset (within about 8 ms of where the time map
//!   asks) that best continues the previous frame. The source past the next onset is read as
//!   silence, so the next hit cannot leak into the body of the piece before it.
//!
//! Material without onsets is a single piece (a verbatim 30 ms start and one long WSOLA body),
//! which is plain WSOLA. At a ratio of 1 every piece is the source itself.

use super::map::TimeMap;
use crate::detect_onsets;

/// Verbatim attack length in milliseconds.
const ATTACK_MS: f64 = 30.0;
/// Crossfade length in milliseconds.
const XFADE_MS: f64 = 2.0;
/// WSOLA frame length in milliseconds.
const FRAME_MS: f64 = 30.0;
/// Onset detector sensitivity (`0..=1`).
const SENSITIVITY: f32 = 0.5;
/// Shortest piece in milliseconds: closer onsets share a piece.
const MIN_PIECE_MS: f64 = 10.0;

/// Source samples with everything at or past `end` read as silence.
struct Masked<'a> {
    chans: &'a [&'a [f32]],
    mono: &'a [f32],
    end: i64,
}

impl Masked<'_> {
    fn mono_at(&self, i: i64) -> f32 {
        if i < 0 || i >= self.end {
            return 0.0;
        }
        self.mono.get(i as usize).copied().unwrap_or(0.0)
    }

    fn chan_at(&self, c: usize, i: i64) -> f32 {
        if i < 0 || i >= self.end {
            return 0.0;
        }
        self.chans[c].get(i as usize).copied().unwrap_or(0.0)
    }
}

/// Where a piece writes: the output range `[start, end)` with a linear fade in over the first
/// `fade_in` frames and a linear fade out over the last `fade_out`.
#[derive(Clone, Copy)]
struct Span {
    start: i64,
    end: i64,
    fade_in: i64,
    fade_out: i64,
}

impl Span {
    fn weight(&self, t: i64) -> f32 {
        let mut w = 1.0f64;
        if self.fade_in > 0 && t < self.start + self.fade_in {
            w *= (t - self.start) as f64 / self.fade_in as f64;
        }
        if self.fade_out > 0 && t >= self.end - self.fade_out {
            w *= (self.end - t) as f64 / self.fade_out as f64;
        }
        w as f32
    }
}

fn ms_frames(ms: f64, sr: u32) -> i64 {
    ((ms * f64::from(sr) / 1000.0).round() as i64).max(1)
}

/// The score of continuing with the frame at `cand` after `target`: the normalised
/// correlation over `len` samples at `stride`.
fn score(m: &Masked, cand: i64, target: i64, len: i64, stride: usize) -> f64 {
    let (mut dot, mut ea, mut eb) = (0.0f64, 0.0f64, 0.0f64);
    let mut i = 0;
    while i < len {
        let (a, b) = (
            f64::from(m.mono_at(cand + i)),
            f64::from(m.mono_at(target + i)),
        );
        dot += a * b;
        ea += a * a;
        eb += b * b;
        i += stride as i64;
    }
    dot / (ea * eb + 1.0e-9).sqrt()
}

/// WSOLA over the output range of `span`, reading the source at `s0 + (t - t0) * slope`.
fn wsola(
    m: &Masked,
    out: &mut [Vec<f32>],
    span: Span,
    (t0, s0): (f64, f64),
    slope: f64,
    sr: u32,
    cancel: &dyn Fn() -> bool,
) -> bool {
    let w_len = (ms_frames(FRAME_MS, sr) & !1).max(64);
    let hs = w_len / 2;
    let reach = hs / 2;
    let window: Vec<f32> = (0..w_len)
        .map(|i| (0.5 - 0.5 * crate::det_trig::cos_cycles(i as f64 / w_len as f64)) as f32)
        .collect();
    let mut prev: Option<i64> = None;
    let mut q = span.start - hs;
    let mut k = 0u32;
    while q < span.end {
        if k.is_multiple_of(8) && cancel() {
            return false;
        }
        k += 1;
        let nominal = (s0 + (q as f64 + hs as f64 - t0) * slope).round() as i64 - hs;
        let a = match prev {
            None => nominal,
            Some(p) => {
                let target = p + hs;
                // Coarse search at a stride of 4, then refine by single samples.
                let mut best = (f64::NEG_INFINITY, nominal);
                let mut c = nominal - reach;
                while c <= nominal + reach {
                    let s = score(m, c, target, hs, 4);
                    if s > best.0 {
                        best = (s, c);
                    }
                    c += 4;
                }
                let centre = best.1;
                for c in centre - 3..=centre + 3 {
                    let s = score(m, c, target, hs, 1);
                    if s > best.0 + 1.0e-12 || (c == centre && s >= best.0) {
                        best = (s, c);
                    }
                }
                best.1
            }
        };
        prev = Some(a);
        for (i, w) in window.iter().enumerate() {
            let t = q + i as i64;
            if t < span.start || t >= span.end {
                continue;
            }
            let g = *w * span.weight(t);
            for (c, o) in out.iter_mut().enumerate() {
                o[t as usize] += m.chan_at(c, a + i as i64) * g;
            }
        }
        q += hs;
    }
    true
}

pub(crate) fn beats(
    source: &[&[f32]],
    map: &TimeMap,
    out_frames: usize,
    sample_rate: u32,
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<Vec<f32>>> {
    let chans: Vec<&[f32]> = source.iter().take(2).copied().collect();
    let mut out: Vec<Vec<f32>> = chans.iter().map(|_| vec![0.0; out_frames]).collect();
    let len = chans.first().map_or(0, |c| c.len());
    if len == 0 || out_frames == 0 {
        return Some(out);
    }
    let mono: Vec<f32> = (0..len)
        .map(|i| chans.iter().map(|c| c[i]).sum::<f32>() / chans.len() as f32)
        .collect();
    let onsets = detect_onsets(&[&mono], sample_rate, SENSITIVITY);
    let out_len = out_frames as i64;
    let s_start = map.src_at(0.0);
    let s_end = map.src_at(out_frames as f64);
    let min_piece = ms_frames(MIN_PIECE_MS, sample_rate);
    // Piece boundaries: (source frame, output frame).
    let mut cuts: Vec<(i64, i64)> = vec![(s_start.round() as i64, 0)];
    for o in onsets {
        let of = o as f64;
        if of <= s_start || of >= s_end {
            continue;
        }
        let t = map.out_at(of).round() as i64;
        let last = cuts.last().map_or(0, |c| c.1);
        if t - last >= min_piece && out_len - t >= min_piece {
            cuts.push((o as i64, t));
        }
    }
    cuts.push((s_end.round() as i64, out_len));
    let attack = ms_frames(ATTACK_MS, sample_rate);
    let xf = ms_frames(XFADE_MS, sample_rate);
    let pieces = cuts.len() - 1;
    for (i, pair) in cuts.windows(2).enumerate() {
        if cancel() {
            return None;
        }
        let ((so, to), (se, te)) = (pair[0], pair[1]);
        let last = i + 1 == pieces;
        let first = i == 0;
        let src_len = (se - so).max(0);
        let a = attack.min(te - to).min(src_len);
        if a <= 0 {
            continue;
        }
        let x = xf.min(a / 2);
        let masked = Masked {
            chans: &chans,
            mono: &mono,
            end: se,
        };
        let has_body = te - to > a && src_len > a;
        // The attack, from `x` frames before the onset (a crossfade from the piece before).
        let span = Span {
            start: to - x,
            end: to + a,
            fade_in: if first { 0 } else { x },
            fade_out: if has_body || !last { x } else { 0 },
        };
        for t in span.start.max(0)..span.end.min(out_len) {
            let g = span.weight(t);
            let idx = so - x + (t - span.start);
            for (c, o) in out.iter_mut().enumerate() {
                if idx >= 0 {
                    o[t as usize] += chans[c].get(idx as usize).copied().unwrap_or(0.0) * g;
                }
            }
        }
        if has_body {
            let slope = (src_len - a) as f64 / (te - to - a) as f64;
            let body = Span {
                start: to + a - x,
                end: te,
                fade_in: x,
                fade_out: if last { 0 } else { x },
            };
            let ok = wsola(
                &masked,
                &mut out,
                body,
                ((to + a) as f64, (so + a) as f64),
                slope,
                sample_rate,
                cancel,
            );
            if !ok {
                return None;
            }
        }
    }
    Some(out)
}
