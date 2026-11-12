//! Pitch edits of monophonic audio (a voice, a bass, a lead): a time-varying pitch shift that
//! keeps the formants, by TD-PSOLA (time-domain pitch-synchronous overlap-add), for the Vocal
//! mode of the audio clip editor.
//!
//! **Analysis.** [`vocal_notes`] cuts a take into notes with the monophonic tracker of
//! [`crate::notes::pitched`] and gives each its pitch curve, [`note_pitch`]: the tracker's 5 ms
//! frames over the note and [`PAD_MS`] around it, read from that span alone, so the curve of a
//! note is the same whichever stretch of the file it was read from, and the render
//! ([`shift_notes`]) hears the same curve and centre (the median of the voiced frames inside
//! the note) as the editor draws.
//!
//! **Target.** [`note_offsets`] gives, per frame, the semitones to add: the note's shift minus
//! `drift` of the note's slow deviation from its centre. With `vibrato_keep` the slow deviation
//! is the deviation through two centred moving averages of 250 ms and 167 ms (zero gain at
//! 4 Hz, 6 Hz and above within a few percent), so a vibrato stays while the wander goes; without
//! it the whole deviation is flattened. Both ramp in and out over [`GLIDE_MS`] at the note's
//! edges, so neighbours do not click.
//!
//! **Shift.** Within each voiced stretch of a note, analysis marks sit one period apart on the
//! waveform's positive peaks (the tracker gives the period). Synthesis marks step by the local
//! analysis spacing divided by the target ratio; each takes the grain (an asymmetric Hann window
//! from the previous mark to the next) of the nearest analysis mark, overlap-added and divided by
//! the sum of the windows. At ratio 1 the synthesis marks are the analysis marks and the windows
//! sum to one, so the audio comes back as it was. Grains are not resampled, so the spectral
//! envelope (the formants) stays where it was while the harmonics move. Unvoiced sound is copied
//! unchanged; a voiced stretch fades in and out of its shifted version over one period, and the
//! whole note fades in and out of it over the glide.
//!
//! Everything is offline, deterministic (only `+ - * /`, `sqrt` and the crate's deterministic
//! `exp2`, `log2` and cosine) and cancellable through the callback, polled once per note.

use crate::det_math::exp2_det;
use crate::det_trig::cos_cycles;
use crate::notes::{PitchParams, extract_in_segments, track_pitch};

/// The keys the vocal tracker looks between (E1 to C7).
pub const VOCAL_KEYS: (u8, u8) = (28, 96);
/// The audio read around a note for its pitch curve, in milliseconds.
pub const PAD_MS: u64 = 40;
/// How long a note takes to glide into and out of its edit, in milliseconds.
pub const GLIDE_MS: u64 = 20;
/// The tracker sensitivity of the pitch curves (the note cut has its own).
const CURVE_SENSITIVITY: f32 = 0.5;
/// The shortest note the analysis keeps, in milliseconds.
const MIN_NOTE_MS: u32 = 60;
/// The lengths of the two moving averages of the slow deviation, in milliseconds.
const SLOW_MS: [u64; 2] = [250, 167];

/// The mono mix the analysis and the render read (the mean of the channels, non-finite samples
/// as silence), so both hear the same curve.
pub fn mono(channels: &[&[f32]]) -> Vec<f32> {
    crate::notes::mono_mix(channels)
}

/// The pitch curve of one note: the tracker's frames over the note and the pad around it.
#[derive(Debug, Clone, PartialEq)]
pub struct NotePitch {
    /// Frames between two entries of [`Self::keys`].
    pub step: u64,
    /// The frame (in the frames of the audio it was read from) entry 0 is centred on.
    pub first: u64,
    /// Per entry: the pitch as a fractional MIDI key, `None` where unvoiced.
    pub keys: Vec<Option<f32>>,
    /// The median pitch of the voiced entries centred inside the note, `None` when none is.
    pub centre: Option<f32>,
}

impl NotePitch {
    /// The frame entry `k` is centred on.
    pub fn frame_of(&self, k: usize) -> u64 {
        self.first + k as u64 * self.step
    }

    /// The same curve with every frame moved by `delta` (to asset frames, say).
    pub fn shifted(mut self, delta: u64) -> Self {
        self.first += delta;
        self
    }
}

/// One note of a take, as [`vocal_notes`] finds it.
#[derive(Debug, Clone, PartialEq)]
pub struct VocalNote {
    /// First frame.
    pub start: u64,
    /// The frame after the last.
    pub end: u64,
    /// The key the tracker rounds the note to.
    pub key: u8,
    /// Peak level (linear).
    pub level: f32,
    /// The pitch curve.
    pub pitch: NotePitch,
}

/// The edit [`shift_notes`] applies to one note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteShift {
    /// First frame of the note.
    pub start: u64,
    /// The frame after its last.
    pub end: u64,
    /// The shift in cents.
    pub shift_cents: i32,
    /// `0..=1`: how much of the slow deviation from the centre is flattened.
    pub drift: f32,
    /// Keep the deviation faster than about 4 Hz while flattening.
    pub vibrato_keep: bool,
}

/// Frames of `ms` milliseconds at `sample_rate`.
fn ms_frames(ms: u64, sample_rate: u32) -> u64 {
    (ms * u64::from(sample_rate) / 1000).max(1)
}

fn median(v: &mut [f32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    })
}

/// The pitch curve of the note `[start, end)` of `mono` (sampled at `sample_rate`): the tracker
/// run on the note and [`PAD_MS`] on both sides (clipped to the audio).
pub fn note_pitch(mono: &[f32], sample_rate: u32, (start, end): (u64, u64)) -> NotePitch {
    let pad = ms_frames(PAD_MS, sample_rate);
    let len = mono.len() as u64;
    let lo = start.saturating_sub(pad).min(len);
    let hi = end.saturating_add(pad).min(len).max(lo);
    let track = track_pitch(
        &[&mono[lo as usize..hi as usize]],
        sample_rate,
        VOCAL_KEYS,
        CURVE_SENSITIVITY,
    );
    let mut inside: Vec<f32> = track
        .keys
        .iter()
        .enumerate()
        .filter_map(|(k, p)| {
            let at = lo + k as u64 * track.step;
            p.filter(|_| at >= start && at < end)
        })
        .collect();
    NotePitch {
        step: track.step.max(1),
        first: lo,
        keys: track.keys,
        centre: median(&mut inside),
    }
}

/// The notes of the monophonic audio `mono` between frames `lo` and `hi`, each with its pitch
/// curve (read from all of `mono`, so give it [`PAD_MS`] beyond the span where there is audio).
/// `sensitivity` is as in [`PitchParams`]. `progress` is told the fraction done (`0..=1`) and
/// stops the analysis when it answers `false` (the result is then `None`).
pub fn vocal_notes(
    mono: &[f32],
    sample_rate: u32,
    (lo, hi): (u64, u64),
    sensitivity: f32,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Option<Vec<VocalNote>> {
    let len = mono.len() as u64;
    let (lo, hi) = (lo.min(len), hi.min(len).max(lo.min(len)));
    let total = hi - lo;
    let params = PitchParams {
        sensitivity,
        min_note_ms: MIN_NOTE_MS,
        key_lo: VOCAL_KEYS.0,
        key_hi: VOCAL_KEYS.1,
        tolerance_cents: 70,
        bend: false,
    };
    let mut read = |at: u64, n: u64| {
        let a = (lo + at).min(hi) as usize;
        let b = (lo + at + n).min(hi) as usize;
        mono[a..b].to_vec()
    };
    let mut cut = |done: u64| progress(0.5 * done as f32 / total.max(1) as f32);
    let found = extract_in_segments(
        total,
        sample_rate,
        crate::notes::Analysis::Pitched(&params),
        &mut read,
        &mut cut,
    )?;
    let count = found.len().max(1);
    let mut out = Vec::with_capacity(found.len());
    for (i, n) in found.into_iter().enumerate() {
        if !progress(0.5 + 0.5 * i as f32 / count as f32) {
            return None;
        }
        let (start, end) = (lo + n.start, lo + n.start + n.length);
        out.push(VocalNote {
            start,
            end,
            key: n.key,
            level: n.level,
            pitch: note_pitch(mono, sample_rate, (start, end)),
        });
    }
    progress(1.0).then_some(out)
}

/// Centred moving average of `v` over `len` entries, the ends held.
fn moving_average(v: &[f32], len: usize) -> Vec<f32> {
    let n = v.len();
    if n == 0 || len <= 1 {
        return v.to_vec();
    }
    let half = len / 2;
    let at = |i: isize| v[i.clamp(0, n as isize - 1) as usize];
    let mut out = Vec::with_capacity(n);
    for i in 0..n as isize {
        let mut sum = 0.0f64;
        for j in (i - half as isize)..(i - half as isize + len as isize) {
            sum += f64::from(at(j));
        }
        out.push((sum / len as f64) as f32);
    }
    out
}

/// The semitones [`shift_notes`] adds at each entry of `pitch`, the curve of the note `edit`:
/// see the module docs. Entries outside the note get 0; so does every entry of a note without a
/// voiced frame when it is only flattened.
pub fn note_offsets(pitch: &NotePitch, edit: &NoteShift, sample_rate: u32) -> Vec<f32> {
    let n = pitch.keys.len();
    let glide = ms_frames(GLIDE_MS, sample_rate) as f64;
    let shift = edit.shift_cents as f32 / 100.0;
    let drift = edit.drift.clamp(0.0, 1.0);
    // The deviation from the centre, unvoiced entries filled from their voiced neighbours.
    let mut dev = vec![0.0f32; n];
    if let Some(centre) = pitch.centre.filter(|_| drift > 0.0) {
        let voiced: Vec<(usize, f32)> = pitch
            .keys
            .iter()
            .enumerate()
            .filter_map(|(k, p)| p.map(|p| (k, p - centre)))
            .collect();
        if !voiced.is_empty() {
            let mut j = 0;
            for (k, d) in dev.iter_mut().enumerate() {
                while j + 1 < voiced.len() && voiced[j + 1].0 <= k {
                    j += 1;
                }
                let (ka, da) = voiced[j];
                *d = match voiced.get(j + 1) {
                    Some(&(kb, db)) if ka <= k => {
                        da + (db - da) * (k - ka) as f32 / (kb - ka) as f32
                    }
                    _ => da,
                };
            }
        }
        if edit.vibrato_keep {
            // The two averages, twiced (`2 S - S S`) so the slow part passes nearly whole.
            let hop_ms = pitch.step as f64 * 1000.0 / f64::from(sample_rate.max(1));
            let smooth = |v: &[f32]| {
                let mut v = v.to_vec();
                for ms in SLOW_MS {
                    let len = ((ms as f64 / hop_ms).round() as usize).max(1);
                    v = moving_average(&v, len);
                }
                v
            };
            let once = smooth(&dev);
            let twice = smooth(&once);
            dev = once.iter().zip(&twice).map(|(a, b)| 2.0 * a - b).collect();
        }
    }
    (0..n)
        .map(|k| {
            let at = pitch.frame_of(k);
            if at < edit.start || at >= edit.end {
                return 0.0;
            }
            let edge = (at - edit.start).min(edit.end - 1 - at) as f64;
            let g = (edge / glide).min(1.0) as f32;
            g * (shift - drift * dev[k])
        })
        .collect()
}

/// Linear interpolation of per-entry values at `frame`, held at the ends.
fn at_frame(values: &[f32], first: u64, step: u64, frame: f64) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let pos = (frame - first as f64) / step as f64;
    if pos <= 0.0 {
        return values[0];
    }
    let i = pos.floor() as usize;
    if i + 1 >= values.len() {
        return values[values.len() - 1];
    }
    let t = (pos - i as f64) as f32;
    values[i] + (values[i + 1] - values[i]) * t
}

/// The positive peak of `x` in `[a, b)` (the first of equal ones).
fn peak(x: &[f32], a: usize, b: usize) -> usize {
    let mut best = a;
    for i in a..b.min(x.len()) {
        if x[i] > x[best] {
            best = i;
        }
    }
    best
}

/// Applies the edits to `source` (one or more channels of equal length, frames counted from
/// the start of `source`): see the module docs. Returns the edited channels (as many as given),
/// or `None` when `cancel` returned true. Frames no edit covers are copied unchanged, and an
/// edit with no shift and no flattening changes nothing.
pub fn shift_notes(
    source: &[&[f32]],
    sample_rate: u32,
    edits: &[NoteShift],
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<Vec<f32>>> {
    let mut out: Vec<Vec<f32>> = source.iter().map(|c| c.to_vec()).collect();
    if edits.iter().all(|e| e.shift_cents == 0 && e.drift <= 0.0) || sample_rate == 0 {
        return Some(out);
    }
    let x = mono(source);
    for edit in edits {
        if cancel() {
            return None;
        }
        if edit.shift_cents == 0 && edit.drift <= 0.0 || edit.end <= edit.start {
            continue;
        }
        let pitch = note_pitch(&x, sample_rate, (edit.start, edit.end));
        let offsets = note_offsets(&pitch, edit, sample_rate);
        if offsets.iter().all(|o| o.abs() < 1.0e-6) {
            continue;
        }
        shift_one(source, &mut out, &x, sample_rate, edit, &pitch, &offsets);
    }
    Some(out)
}

/// The voiced stretches of a curve as `[first, last]` entry indexes (at least three entries).
fn voiced_runs(keys: &[Option<f32>]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut k = 0;
    while k < keys.len() {
        if keys[k].is_none() {
            k += 1;
            continue;
        }
        let a = k;
        while k < keys.len() && keys[k].is_some() {
            k += 1;
        }
        if k - a >= 3 {
            runs.push((a, k - 1));
        }
    }
    runs
}

/// Shifts one note into `out` (see [`shift_notes`]).
fn shift_one(
    source: &[&[f32]],
    out: &mut [Vec<f32>],
    x: &[f32],
    sample_rate: u32,
    edit: &NoteShift,
    pitch: &NotePitch,
    offsets: &[f32],
) {
    let sr = f64::from(sample_rate);
    let len = x.len();
    let glide = ms_frames(GLIDE_MS, sample_rate) as f64;
    // The note's own fade into its shifted version.
    let note_gain = |t: usize| -> f32 {
        let t = t as u64;
        if t < edit.start || t >= edit.end {
            return 0.0;
        }
        let edge = (t - edit.start).min(edit.end - 1 - t) as f64;
        (edge / glide).min(1.0) as f32
    };
    let keys: Vec<f32> = pitch.keys.iter().map(|p| p.unwrap_or(0.0)).collect();
    for (ka, kb) in voiced_runs(&pitch.keys) {
        let (fa, fb) = (pitch.frame_of(ka), pitch.frame_of(kb));
        // Only a stretch that overlaps the note is touched.
        if fb < edit.start || fa >= edit.end {
            continue;
        }
        let a = fa.saturating_sub(pitch.step / 2) as usize;
        let b = ((fb + pitch.step / 2) as usize).min(len);
        if b <= a + 8 {
            continue;
        }
        let run_keys = &keys[ka..=kb];
        let period = |t: f64| -> f64 {
            let key = at_frame(run_keys, fa, pitch.step, t);
            sr / (440.0 * f64::from(exp2_det((key - 69.0) / 12.0)))
        };
        let ratio = |t: f64| -> f64 {
            f64::from(exp2_det(
                at_frame(offsets, pitch.first, pitch.step, t) / 12.0,
            ))
        };
        // Analysis marks on the positive peaks, a period apart.
        let p0 = period(a as f64).max(2.0);
        let mut marks = vec![peak(x, a, (a + p0 as usize + 1).min(b))];
        loop {
            let t = *marks.last().unwrap_or(&a);
            let p = period(t as f64).max(2.0);
            let pred = t as f64 + p;
            if pred >= b as f64 {
                break;
            }
            let lo = (pred - p / 4.0).max(t as f64 + 1.0) as usize;
            let hi = ((pred + p / 4.0) as usize + 1).min(b);
            if hi <= lo {
                break;
            }
            marks.push(peak(x, lo, hi));
        }
        if marks.len() < 3 {
            continue;
        }
        // Synthesis: grains of the nearest analysis mark at the target spacing.
        let first = marks[0];
        let last = marks[marks.len() - 1];
        let span = last + (last - marks[marks.len() - 2]) + 1;
        let base = first.saturating_sub(marks[1] - marks[0]);
        let width = span.min(len) - base;
        let channels = source.len();
        let mut acc = vec![vec![0.0f64; width]; channels];
        let mut wsum = vec![0.0f64; width];
        let mut ts = first as f64;
        let (mut j, mut g) = (0usize, 0usize);
        while ts <= last as f64 {
            while j + 1 < marks.len() && marks[j + 1] as f64 <= ts {
                j += 1;
            }
            while g + 1 < marks.len()
                && (marks[g + 1] as f64 - ts).abs() <= (marks[g] as f64 - ts).abs()
            {
                g += 1;
            }
            let at = ts.round() as usize;
            let centre = marks[g];
            let left = if g > 0 {
                centre - marks[g - 1]
            } else {
                marks[1] - marks[0]
            };
            let right = if g + 1 < marks.len() {
                marks[g + 1] - centre
            } else {
                centre - marks[g - 1]
            };
            for u in -(left as isize)..=(right as isize) {
                let w = if u <= 0 {
                    0.5 - 0.5 * cos_cycles(0.5 * (u + left as isize) as f64 / left as f64)
                } else {
                    0.5 + 0.5 * cos_cycles(0.5 * u as f64 / right as f64)
                };
                let (src, dst) = (centre as isize + u, at as isize + u);
                if src < 0 || src as usize >= len || dst < base as isize {
                    continue;
                }
                let d = dst as usize - base;
                if d >= width {
                    continue;
                }
                wsum[d] += w;
                for (c, ch) in source.iter().enumerate() {
                    acc[c][d] += w * f64::from(ch[src as usize]);
                }
            }
            let step = if j + 1 < marks.len() {
                marks[j + 1] - marks[j]
            } else {
                marks[j] - marks[j - 1]
            } as f64;
            ts += (step / ratio(ts)).max(1.0);
        }
        // Into the output: faded in and out over one period at the stretch's ends, and by the
        // note's glide.
        let fade = (marks[1] - marks[0]).max(1) as f64;
        for d in 0..width {
            let t = base + d;
            if wsum[d] < 1.0e-3 || t < first || t > last {
                continue;
            }
            let edge = ((t - first).min(last - t) as f64 / fade).min(1.0) as f32;
            let mix = edge * note_gain(t);
            if mix <= 0.0 {
                continue;
            }
            for (c, ch) in source.iter().enumerate() {
                let y = (acc[c][d] / wsum[d]) as f32;
                out[c][t] += mix * (y - ch[t]);
            }
        }
    }
}
