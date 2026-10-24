//! Audio clip editing helpers: gain envelope slicing, peak and normalise, and the strip-silence
//! detector. All of it is plain arithmetic on frames and samples; the document commands that
//! use it (`SplitClip`, `StripSilence`) and the app (which owns the sample data) call it.

use crate::song::{AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN, GainPoint, envelope_gain};

/// The envelope value in dB at frame `n`: a point's own value when `n` is on a point, else
/// the amplitude ramp between the neighbours read back as dB (held outside the points).
fn db_at(points: &[GainPoint], n: u64) -> f32 {
    if let Some(p) = points.iter().find(|p| p.frame == n) {
        return p.gain_db;
    }
    let linear: Vec<(u64, f32)> = points
        .iter()
        .map(|p| (p.frame, atomik_core::db_to_gain(p.gain_db)))
        .collect();
    let amp = envelope_gain(&linear, n).unwrap_or(1.0);
    atomik_core::gain_to_db(amp).clamp(AUDIO_GAIN_DB_MIN, AUDIO_GAIN_DB_MAX)
}

/// The envelope of a clip that keeps the frames `[from, to)` of this one, re-based so that
/// `from` is frame 0: the points inside, plus a point at the new start (unless `from` is 0 or
/// on a point) and at `to - from` when later points exist, carrying the value the original
/// envelope has there, so the slice sounds as the original did over that span. An empty
/// envelope stays empty.
pub fn slice_envelope(points: &[GainPoint], from: u64, to: u64) -> Vec<GainPoint> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if from > 0 && !points.iter().any(|p| p.frame == from) {
        out.push(GainPoint {
            frame: 0,
            gain_db: db_at(points, from),
        });
    }
    out.extend(
        points
            .iter()
            .filter(|p| p.frame >= from && p.frame < to)
            .map(|p| GainPoint {
                frame: p.frame - from,
                gain_db: p.gain_db,
            }),
    );
    if to != u64::MAX
        && points.iter().any(|p| p.frame >= to)
        && !points.iter().any(|p| p.frame == to)
    {
        out.push(GainPoint {
            frame: to - from,
            gain_db: db_at(points, to),
        });
    } else if let Some(p) = points.iter().find(|p| p.frame == to && to != u64::MAX) {
        out.push(GainPoint {
            frame: to - from,
            gain_db: p.gain_db,
        });
    }
    out
}

/// The envelopes of the two halves of a clip split `at` frames from its start, as
/// `(right, left)`.
pub fn split_envelope(points: &[GainPoint], at: u64) -> (Vec<GainPoint>, Vec<GainPoint>) {
    (
        slice_envelope(points, at, u64::MAX),
        slice_envelope(points, 0, at),
    )
}

/// The largest absolute sample over the frames `[from, to)` of every channel (frames past a
/// channel's end count as silence).
pub fn peak<C: AsRef<[f32]>>(channels: &[C], from: u64, to: u64) -> f32 {
    let mut peak = 0.0f32;
    for c in channels {
        let c = c.as_ref();
        let lo = usize::try_from(from).unwrap_or(usize::MAX).min(c.len());
        let hi = usize::try_from(to).unwrap_or(usize::MAX).min(c.len());
        for &s in &c[lo..hi.max(lo)] {
            let a = s.abs();
            if a > peak {
                peak = a;
            }
        }
    }
    peak
}

/// The clip gain in dB that brings a region of peak `peak` (linear, before any gain) to
/// `target_db` dBFS, clamped to the clip gain range; `None` for a silent or non-finite peak.
pub fn normalise_gain_db(peak: f32, target_db: f32) -> Option<f32> {
    if !(peak.is_finite() && peak > 0.0 && target_db.is_finite()) {
        return None;
    }
    Some((target_db - 20.0 * peak.log10()).clamp(AUDIO_GAIN_DB_MIN, AUDIO_GAIN_DB_MAX))
}

/// What counts as silence to strip, and what to keep around the rest. Lengths are in frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripParams {
    /// A frame is silent when every channel is below this level (dBFS).
    pub threshold_db: f32,
    /// A silent run is removed only when at least this long (`>= 1`).
    pub min_silence: u64,
    /// Frames kept before each piece (the silence just before the sound).
    pub pad_before: u64,
    /// Frames kept after each piece.
    pub pad_after: u64,
}

/// The pieces of `[from, to)` that stay after stripping silence, as frame ranges relative to
/// `from`, ascending and disjoint.
///
/// A silent run of at least `min_silence` frames is cut out (so are a silent start and end);
/// every piece that remains grows by `pad_before` frames at its start and `pad_after` at its
/// end (never past `[from, to)`), and pieces that then touch or overlap join. The result is a
/// single range covering everything when there is no silence to strip, and empty when the
/// whole span is silent.
pub fn strip_silence<C: AsRef<[f32]>>(
    channels: &[C],
    from: u64,
    to: u64,
    params: &StripParams,
) -> Vec<(u64, u64)> {
    let len = to.saturating_sub(from);
    if len == 0 {
        return Vec::new();
    }
    let threshold = atomik_core::db_to_gain(params.threshold_db);
    let min_silence = params.min_silence.max(1);
    let loud = |k: u64| {
        let i = usize::try_from(from + k).unwrap_or(usize::MAX);
        channels
            .iter()
            .any(|c| c.as_ref().get(i).is_some_and(|s| s.abs() >= threshold))
    };
    // The kept ranges: everything except the silent runs of at least `min_silence` frames.
    let mut kept: Vec<(u64, u64)> = Vec::new();
    let mut keep_start = 0u64;
    let mut k = 0u64;
    while k < len {
        if loud(k) {
            k += 1;
            continue;
        }
        let run_start = k;
        while k < len && !loud(k) {
            k += 1;
        }
        if k - run_start >= min_silence {
            if run_start > keep_start {
                kept.push((keep_start, run_start));
            }
            keep_start = k;
        }
    }
    if keep_start < len {
        kept.push((keep_start, len));
    }
    // Pad, clamp and join.
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(kept.len());
    for (s, e) in kept {
        let s = s.saturating_sub(params.pad_before);
        let e = e.saturating_add(params.pad_after).min(len);
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}
