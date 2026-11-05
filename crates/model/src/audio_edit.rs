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

/// The envelope of the clip that two clips make when the second one starts `at` frames after the
/// first: the first's envelope up to `at`, the second's from there on (shifted by `at`), each
/// holding its own level where the other one begins, so both parts sound as they did. `None`
/// when the result would have more than [`MAX_GAIN_POINTS`] points. Both empty stays empty; one
/// empty counts as no gain change (0 dB).
pub fn join_envelopes(left: &[GainPoint], right: &[GainPoint], at: u64) -> Option<Vec<GainPoint>> {
    if left.is_empty() && right.is_empty() {
        return Some(Vec::new());
    }
    let mut out: Vec<GainPoint> = Vec::new();
    if at > 0 {
        if left.is_empty() {
            out.push(GainPoint {
                frame: 0,
                gain_db: 0.0,
            });
        } else {
            out.extend(left.iter().filter(|p| p.frame < at).copied());
            if out.last().is_none_or(|p| p.frame < at - 1) {
                out.push(GainPoint {
                    frame: at - 1,
                    gain_db: db_at(left, at - 1),
                });
            }
        }
    }
    if right.is_empty() {
        out.push(GainPoint {
            frame: at,
            gain_db: 0.0,
        });
    } else {
        if right[0].frame > 0 {
            out.push(GainPoint {
                frame: at,
                gain_db: right[0].gain_db,
            });
        }
        out.extend(right.iter().map(|p| GainPoint {
            frame: p.frame + at,
            gain_db: p.gain_db,
        }));
    }
    (out.len() <= crate::song::MAX_GAIN_POINTS).then_some(out)
}

/// The envelope of a clip whose start moves `by` frames earlier (it now begins `by` frames
/// before the old start): the old points shifted later, with the first level held over the new
/// part. An empty envelope stays empty; `None` when it would have too many points.
pub fn extend_envelope_start(points: &[GainPoint], by: u64) -> Option<Vec<GainPoint>> {
    if points.is_empty() || by == 0 {
        return Some(points.to_vec());
    }
    let mut out = vec![GainPoint {
        frame: 0,
        gain_db: points[0].gain_db,
    }];
    out.extend(points.iter().map(|p| GainPoint {
        frame: p.frame + by,
        gain_db: p.gain_db,
    }));
    (out.len() <= crate::song::MAX_GAIN_POINTS).then_some(out)
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

/// What a normalise measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormaliseMode {
    /// The largest sample, in dBFS.
    Peak,
    /// The root mean square of all channels over the span, in dBFS.
    Rms,
    /// Integrated loudness (BS.1770 K-weighted, gated), in LUFS.
    Loudness,
}

/// A normalise target: the level the clip should reach, in the unit of `mode`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormaliseParams {
    /// dBFS for peak and RMS, LUFS for loudness.
    pub target_db: f32,
    /// What is measured.
    pub mode: NormaliseMode,
}

impl Default for NormaliseParams {
    /// Peak to 0 dBFS, what Normalise Audio always did.
    fn default() -> Self {
        Self {
            target_db: 0.0,
            mode: NormaliseMode::Peak,
        }
    }
}

/// The root mean square level in dBFS of the frames `[from, to)` of every channel, or `None`
/// for silence (no frames, or all zero). Frames past a channel's end count as silence.
pub fn rms_db<C: AsRef<[f32]>>(channels: &[C], from: u64, to: u64) -> Option<f32> {
    let (mut sum, mut n) = (0.0f64, 0u64);
    for c in channels {
        let c = c.as_ref();
        let lo = usize::try_from(from).unwrap_or(usize::MAX).min(c.len());
        let hi = usize::try_from(to).unwrap_or(usize::MAX).min(c.len());
        for &x in &c[lo..hi.max(lo)] {
            sum += f64::from(x) * f64::from(x);
        }
        n += (hi.max(lo) - lo) as u64;
    }
    (n > 0 && sum > 0.0 && sum.is_finite()).then(|| (10.0 * (sum / n as f64).log10()) as f32)
}

/// The clip gain in dB that moves a measured `level_db` to `target_db`, clamped to the clip gain
/// range; `None` for a non-finite level or target.
pub fn gain_to_level_db(level_db: f32, target_db: f32) -> Option<f32> {
    (level_db.is_finite() && target_db.is_finite())
        .then(|| (target_db - level_db).clamp(AUDIO_GAIN_DB_MIN, AUDIO_GAIN_DB_MAX))
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

/// The silence detector of [`strip_silence`] fed in blocks, so a span of any length is read
/// with memory that does not grow with it: the state is a few counters and the ranges found
/// so far.
///
/// Create it with the span's length in frames, [`push`](Self::push) the blocks in order (each a
/// list of planar channels, any length, the channels may differ in length) and call
/// [`finish`](Self::finish) for the kept ranges. Frames the blocks do not supply, up to the
/// span's length, count as silence, and so does everything past it. The result is the same
/// whatever the block sizes.
#[derive(Debug, Clone)]
pub struct StripScan {
    len: u64,
    threshold: f32,
    min_silence: u64,
    pad_before: u64,
    pad_after: u64,
    /// Frames consumed so far (the next frame's position).
    at: u64,
    /// Where the current silent run began, if one is open.
    run: Option<u64>,
    /// Where the piece being kept begins.
    keep_start: u64,
    kept: Vec<(u64, u64)>,
}

impl StripScan {
    /// A detector for a span of `len` frames.
    pub fn new(len: u64, params: &StripParams) -> Self {
        Self {
            len,
            threshold: atomik_core::db_to_gain(params.threshold_db),
            min_silence: params.min_silence.max(1),
            pad_before: params.pad_before,
            pad_after: params.pad_after,
            at: 0,
            run: None,
            keep_start: 0,
            kept: Vec::new(),
        }
    }

    /// Closes the silent run `[start, end)`: a long enough one ends the piece before it.
    fn close_run(&mut self, start: u64, end: u64) {
        if end - start >= self.min_silence {
            if start > self.keep_start {
                self.kept.push((self.keep_start, start));
            }
            self.keep_start = end;
        }
    }

    /// The next block of frames: a frame is loud when any channel reaches the threshold there.
    pub fn push<C: AsRef<[f32]>>(&mut self, block: &[C]) {
        let frames = block.iter().map(|c| c.as_ref().len()).max().unwrap_or(0) as u64;
        let take = frames.min(self.len.saturating_sub(self.at));
        for k in 0..take as usize {
            let loud = block
                .iter()
                .any(|c| c.as_ref().get(k).is_some_and(|s| s.abs() >= self.threshold));
            if loud {
                if let Some(start) = self.run.take() {
                    self.close_run(start, self.at);
                }
            } else if self.run.is_none() {
                self.run = Some(self.at);
            }
            self.at += 1;
        }
    }

    /// The kept ranges, relative to the span's start, ascending and disjoint (see
    /// [`strip_silence`]).
    pub fn finish(mut self) -> Vec<(u64, u64)> {
        let len = self.len;
        if len == 0 {
            return Vec::new();
        }
        // What was not supplied is silent.
        if self.at < len && self.run.is_none() {
            self.run = Some(self.at);
        }
        if let Some(start) = self.run.take() {
            self.close_run(start, len);
        }
        if self.keep_start < len {
            self.kept.push((self.keep_start, len));
        }
        // Pad, clamp and join.
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(self.kept.len());
        for &(s, e) in &self.kept {
            let s = s.saturating_sub(self.pad_before);
            let e = e.saturating_add(self.pad_after).min(len);
            match out.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => out.push((s, e)),
            }
        }
        out
    }
}

/// The pieces of `[from, to)` that stay after stripping silence, as frame ranges relative to
/// `from`, ascending and disjoint.
///
/// A silent run of at least `min_silence` frames is cut out (so are a silent start and end);
/// every piece that remains grows by `pad_before` frames at its start and `pad_after` at its
/// end (never past `[from, to)`), and pieces that then touch or overlap join. The result is a
/// single range covering everything when there is no silence to strip, and empty when the
/// whole span is silent. Frames past a channel's end count as silence. This reads channels
/// that are already in memory; [`StripScan`] is the same detector for audio read in blocks.
pub fn strip_silence<C: AsRef<[f32]>>(
    channels: &[C],
    from: u64,
    to: u64,
    params: &StripParams,
) -> Vec<(u64, u64)> {
    let len = to.saturating_sub(from);
    let mut scan = StripScan::new(len, params);
    let lo = usize::try_from(from).unwrap_or(usize::MAX);
    let hi = usize::try_from(to).unwrap_or(usize::MAX);
    let span: Vec<&[f32]> = channels
        .iter()
        .map(|c| {
            let c = c.as_ref();
            &c[lo.min(c.len())..hi.min(c.len()).max(lo.min(c.len()))]
        })
        .collect();
    scan.push(&span);
    scan.finish()
}

/// The first frame whose level reaches `threshold_db` on any channel: how much silence a
/// recording starts with, found by the detector of [`strip_silence`]. `None` when the whole
/// recording is quieter than that.
pub fn leading_silence<C: AsRef<[f32]>>(channels: &[C], threshold_db: f32) -> Option<u64> {
    let len = channels.iter().map(|c| c.as_ref().len()).max().unwrap_or(0) as u64;
    let params = StripParams {
        threshold_db,
        min_silence: 1,
        pad_before: 0,
        pad_after: 0,
    };
    strip_silence(channels, 0, len, &params)
        .first()
        .map(|&(start, _)| start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_silence_is_the_first_frame_at_the_threshold() {
        let mut left = vec![0.0f32; 100];
        left[40] = 0.5;
        let mut right = vec![0.0f32; 100];
        right[25] = -0.25;
        assert_eq!(leading_silence(&[&left, &right], -40.0), Some(25));
        assert_eq!(leading_silence(&[&left], -40.0), Some(40));
        // Quieter than the threshold counts as silence.
        let faint = vec![0.0001f32; 10];
        assert_eq!(leading_silence(&[&faint], -40.0), None);
        assert_eq!(leading_silence(&[&faint], -90.0), Some(0));
        assert_eq!(leading_silence::<Vec<f32>>(&[], -40.0), None);
    }

    /// The detector as it was before blocks: every channel in memory, one frame at a time.
    fn reference(channels: &[Vec<f32>], from: u64, to: u64, p: &StripParams) -> Vec<(u64, u64)> {
        let len = to.saturating_sub(from);
        if len == 0 {
            return Vec::new();
        }
        let threshold = atomik_core::db_to_gain(p.threshold_db);
        let min_silence = p.min_silence.max(1);
        let loud = |k: u64| {
            let i = usize::try_from(from + k).unwrap_or(usize::MAX);
            channels
                .iter()
                .any(|c| c.get(i).is_some_and(|s| s.abs() >= threshold))
        };
        let mut kept: Vec<(u64, u64)> = Vec::new();
        let (mut keep_start, mut k) = (0u64, 0u64);
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
        let mut out: Vec<(u64, u64)> = Vec::new();
        for (s, e) in kept {
            let s = s.saturating_sub(p.pad_before);
            let e = e.saturating_add(p.pad_after).min(len);
            match out.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => out.push((s, e)),
            }
        }
        out
    }

    /// Sparse bursts and gaps from a fixed generator.
    fn noise(seed: u64, frames: usize, channels: usize) -> Vec<Vec<f32>> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        (0..channels)
            .map(|_| {
                let mut out = vec![0.0f32; frames];
                let mut at = 0;
                while at < frames {
                    let gap = (next() % 90) as usize;
                    let burst = (next() % 40) as usize + 1;
                    at += gap;
                    for s in out.iter_mut().skip(at).take(burst) {
                        *s = if next() % 2 == 0 { 0.4 } else { -0.3 };
                    }
                    at += burst;
                }
                out
            })
            .collect()
    }

    #[test]
    fn the_streaming_scan_agrees_with_the_whole_span_detector_at_any_block_size() {
        for seed in 1..=24u64 {
            let frames = 700 + (seed as usize * 37) % 400;
            let channels = noise(seed, frames, 1 + (seed as usize % 2));
            for (min_silence, pad_before, pad_after) in
                [(1, 0, 0), (20, 0, 0), (35, 5, 9), (60, 30, 30), (500, 3, 3)]
            {
                let params = StripParams {
                    threshold_db: -20.0,
                    min_silence,
                    pad_before,
                    pad_after,
                };
                let (from, to) = (seed % 7, frames as u64 + 20);
                let want = reference(&channels, from, to, &params);
                assert_eq!(
                    strip_silence(&channels, from, to, &params),
                    want,
                    "seed {seed}"
                );
                for block in [1usize, 2, 7, 64, 333, 100_000] {
                    let mut scan = StripScan::new(to - from, &params);
                    let mut at = from as usize;
                    while at < frames {
                        let end = (at + block).min(frames);
                        let part: Vec<&[f32]> = channels.iter().map(|c| &c[at..end]).collect();
                        scan.push(&part);
                        at = end;
                    }
                    assert_eq!(scan.finish(), want, "seed {seed} block {block} {params:?}");
                }
            }
        }
    }

    #[test]
    fn the_streaming_scan_counts_missing_frames_as_silence_and_keeps_little_state() {
        let params = StripParams {
            threshold_db: -20.0,
            min_silence: 10,
            pad_before: 0,
            pad_after: 0,
        };
        // Sound for 30 frames, then the source ends but the span goes on to 200.
        let mut scan = StripScan::new(200, &params);
        scan.push(&[vec![0.5f32; 30]]);
        assert_eq!(scan.finish(), vec![(0, 30)]);
        // Nothing supplied at all: the whole span is silent.
        assert!(StripScan::new(50, &params).finish().is_empty());
        assert!(StripScan::new(0, &params).finish().is_empty());
        // Millions of frames of silence in blocks never grow the state.
        let mut scan = StripScan::new(4_194_304, &params);
        let block = vec![0.0f32; 1 << 16];
        for _ in 0..(4_194_304u64 / (1 << 16)) {
            scan.push(&[&block]);
        }
        assert!(scan.kept.is_empty());
        assert!(scan.finish().is_empty());
    }
}
