//! The offline separation: chunks with a crossfade, the harmonic/percussive split, the band and
//! stereo masks, the inverse transform and the streaming of the finished samples to a
//! [`StemSink`].
//!
//! # The method
//!
//! Each chunk is about twenty seconds and is analysed with a second of context on each side, so
//! the filters have settled by the time the samples that are kept begin. Its transform is a
//! 2048-point Hann-windowed STFT with a hop of 512 (46 ms windows, 11.6 ms hop, 21.5 Hz bins).
//!
//! 1. The magnitude of every cell is the power mean of the two channels. A median filter of 17
//!    frames (about 200 ms) across time keeps what lasts (the harmonic estimate `H`); a median
//!    filter of 17 bins (about 370 Hz) across frequency keeps what is broadband and short (the
//!    percussive estimate `P`). The soft mask of the drums is `P^2 / (H^2 + P^2)`, so the drums
//!    and the harmonic rest `1 - mask` always add up.
//! 2. The harmonic rest is split by frequency: a smooth crossover from 100 Hz to 200 Hz (centred
//!    on 150 Hz, a smoothstep) gives the bass below and the voice band above.
//! 3. The vocals are the harmonic rest above the crossover that is centred: a cell counts as
//!    centred when the left and right values are alike (twice their inner product over the sum of
//!    their powers, squared, zero when they are out of phase), and a vocal-range weight keeps the
//!    band of the voice at full strength (up to 5 kHz) and lets the top fall to a third. A mono
//!    input has no stereo image to tell the voice from the rest: its vocals are the harmonic band
//!    from 200 Hz to 5 kHz at a weight of 0.6, and the dialog says so.
//! 4. The other stem is what is left: the mix minus the three others in the time domain, so the
//!    four stems add up to the mix to the rounding of a float.
//!
//! Neighbouring chunks are crossfaded linearly over the second around their boundary; the
//! weights add up to one, so the sum of the stems stays the mix across boundaries too.
//!
//! The median passes are split over frame ranges on a few threads; every cell is computed from
//! the same inputs whichever thread runs it, so the output does not depend on the thread count.

use crate::stft::{Spectrogram, Stft};
use crate::{Control, RATE, Stem, StemError};
use std::sync::atomic::AtomicBool;

/// The seconds of signal a chunk keeps (not counting its context).
pub const CHUNK_SECONDS: f64 = 20.0;
/// The seconds of context on each side of a chunk.
const CONTEXT_SECONDS: f64 = 1.0;
/// The size of the transform.
const N_FFT: usize = 2048;
/// The length of the median across time, in frames (odd).
const TIME_MEDIAN: usize = 17;
/// The length of the median across frequency, in bins (odd).
const FREQ_MEDIAN: usize = 17;
const _: () = assert!(
    TIME_MEDIAN == 17 && FREQ_MEDIAN == 17,
    "the scratch window holds 17"
);
/// The bass/voice crossover is a smoothstep between these two frequencies, in Hz.
const CROSSOVER: (f32, f32) = (100.0, 200.0);
/// Above this the voice weight falls (stereo input), in Hz, over [`VOICE_FALL`] Hz to its floor.
const VOICE_TOP: f32 = 5_000.0;
const VOICE_FALL: f32 = 4_000.0;
const VOICE_FLOOR: f32 = 0.34;
/// The weight of the vocals of a mono input, and the end of its band, in Hz.
const MONO_VOICE_WEIGHT: f32 = 0.6;
const MONO_VOICE_TOP: f32 = 5_000.0;
const MONO_VOICE_FALL: f32 = 1_000.0;
/// The most threads a median pass uses.
const MAX_THREADS: usize = 8;

/// The receiver of the finished stems. For every stem the pieces come in order and cover the
/// song once, without gaps.
pub trait StemSink {
    /// The next samples of `stem`, left and right.
    fn write(&mut self, stem: Stem, left: &[f32], right: &[f32]) -> Result<(), StemError>;
}

/// A piece of the song being assembled: the output of one stem and channel between the samples
/// already sent on and the end of the last chunk.
struct Tail {
    /// The first song sample that is held.
    from: usize,
    data: [Vec<f32>; 2],
}

/// Separates a stereo song at [`RATE`] into stems, in chunks of [`CHUNK_SECONDS`].
///
/// `wanted` are the stems sent to the sink. `progress` is called with `0..=1`, going up, several
/// times per chunk; `cancel` stops the run (the result is then `Err(Cancelled)` and the sink has
/// been given a prefix only). Non-finite samples are treated as silence. A longer channel is cut
/// to the shorter one.
///
/// # Errors
/// `NothingWanted` for an empty `wanted`, `Cancelled`, or the sink's own error.
pub fn separate(
    left: &[f32],
    right: &[f32],
    wanted: &[Stem],
    sink: &mut dyn StemSink,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), StemError> {
    separate_in_chunks(left, right, wanted, CHUNK_SECONDS, sink, cancel, progress)
}

/// [`separate`] with chunks of `chunk_seconds` (at least a second; the tests use short ones).
///
/// # Errors
/// As [`separate`].
pub fn separate_in_chunks(
    left: &[f32],
    right: &[f32],
    wanted: &[Stem],
    chunk_seconds: f64,
    sink: &mut dyn StemSink,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), StemError> {
    if wanted.is_empty() {
        return Err(StemError::NothingWanted);
    }
    let control = Control::new(cancel);
    control.check()?;
    let len = left.len().min(right.len());
    let rate = f64::from(RATE);
    let context = (CONTEXT_SECONDS * rate) as usize;
    let core = ((chunk_seconds * rate) as usize).max(context);
    let half = context / 2;
    let chunks = len.div_ceil(core).max(1);
    let mono = (0..len).all(|i| clean(left[i]).to_bits() == clean(right[i]).to_bits());
    let tools = Tools::new(mono);
    let mut tails: Vec<(Stem, Tail)> = wanted
        .iter()
        .map(|&s| {
            (
                s,
                Tail {
                    from: 0,
                    data: [Vec::new(), Vec::new()],
                },
            )
        })
        .collect();
    progress(0.0);
    for i in 0..chunks {
        control.check()?;
        let start = i * core;
        let end = ((i + 1) * core).min(len);
        let a = start.saturating_sub(context);
        let b = (end + context).min(len);
        let mut inner = |p: f32| progress(((i as f32) + p.clamp(0.0, 1.0)) / chunks as f32);
        let pieces = chunk(
            &tools,
            (&left[a..b], &right[a..b]),
            wanted,
            &control,
            &mut inner,
        )?;
        for ((_, tail), piece) in tails.iter_mut().zip(pieces) {
            // Samples before `tail.from` were sent already and weigh nothing here.
            let skip = tail.from.saturating_sub(a);
            for (data, piece) in tail.data.iter_mut().zip(&piece) {
                if data.len() < b - tail.from {
                    data.resize(b - tail.from, 0.0);
                }
                for (j, v) in piece.iter().enumerate().skip(skip) {
                    let pos = a + j;
                    data[pos - tail.from] += v * weight(pos, start, end, len, half);
                }
            }
        }
        // Everything before the next chunk's ramp is final.
        let done = if i + 1 == chunks {
            len
        } else {
            end.saturating_sub(half)
        };
        for (stem, tail) in &mut tails {
            if done > tail.from {
                let n = done - tail.from;
                sink.write(*stem, &tail.data[0][..n], &tail.data[1][..n])?;
                tail.data[0].drain(..n);
                tail.data[1].drain(..n);
                tail.from = done;
            }
        }
        progress((i + 1) as f32 / chunks as f32);
    }
    Ok(())
}

/// How much the chunk `[start, end)` counts at the song sample `pos`.
fn weight(pos: usize, start: usize, end: usize, len: usize, half: usize) -> f32 {
    let mut w = 1.0f32;
    if start > 0 {
        // Rising from `start - half` to `start + half`.
        if pos + half <= start {
            return 0.0;
        }
        if pos < start + half {
            w = w.min((pos + half - start) as f32 / (2 * half) as f32);
        }
    }
    if end < len {
        // Falling from `end - half` to `end + half`.
        if pos >= end + half {
            return 0.0;
        }
        if pos + half > end {
            w = w.min((end + half - pos) as f32 / (2 * half) as f32);
        }
    }
    w
}

/// A non-finite sample is silence.
fn clean(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// `0` below `lo`, `1` above `hi`, a smoothstep between.
fn smooth(x: f32, lo: f32, hi: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The transform and the per-bin weights, made once per run.
struct Tools {
    stft: Stft,
    /// The weight of the bass at each bin (the harmonic part below the crossover).
    bass: Vec<f32>,
    /// The weight of the vocals at each bin (the harmonic part above it, in the voice's range).
    voice: Vec<f32>,
    /// The input is the same on both channels.
    mono: bool,
}

impl Tools {
    fn new(mono: bool) -> Self {
        let stft = Stft::new(N_FFT, Stft::hann(N_FFT));
        let bins = stft.bins();
        let hz = |k: usize| k as f32 * RATE as f32 / N_FFT as f32;
        let bass: Vec<f32> = (0..bins)
            .map(|k| 1.0 - smooth(hz(k), CROSSOVER.0, CROSSOVER.1))
            .collect();
        let voice = (0..bins)
            .map(|k| {
                let above = 1.0 - bass[k];
                if mono {
                    MONO_VOICE_WEIGHT
                        * above
                        * (1.0 - smooth(hz(k), MONO_VOICE_TOP, MONO_VOICE_TOP + MONO_VOICE_FALL))
                } else {
                    above
                        * (1.0
                            - (1.0 - VOICE_FLOOR)
                                * smooth(hz(k), VOICE_TOP, VOICE_TOP + VOICE_FALL))
                }
            })
            .collect();
        Self {
            stft,
            bass,
            voice,
            mono,
        }
    }
}

/// Which of the three filtered stems a masked spectrogram is for.
#[derive(Clone, Copy)]
enum Part {
    Drums,
    Bass,
    Vocals,
}

/// The wanted stems of one stretch of the song (with its context), in the order of `wanted`.
fn chunk(
    tools: &Tools,
    (left, right): (&[f32], &[f32]),
    wanted: &[Stem],
    control: &Control<'_>,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<[Vec<f32>; 2]>, StemError> {
    let len = left.len();
    let l: Vec<f32> = left.iter().map(|v| clean(*v)).collect();
    let r: Vec<f32> = right.iter().map(|v| clean(*v)).collect();
    if l.iter().chain(&r).all(|v| *v == 0.0) {
        return Ok(wanted
            .iter()
            .map(|_| [vec![0f32; len], vec![0f32; len]])
            .collect());
    }
    let spec = tools.stft.analyse(&l, &r);
    progress(0.1);
    control.check()?;
    let mask = percussive_mask(&spec, tools.stft.bins(), control);
    control.check()?;
    progress(0.55);
    // The three filtered stems that are needed; the other one needs them all.
    let has = |s: Stem| wanted.contains(&s);
    let other = has(Stem::Other);
    let need = [
        (Part::Drums, other || has(Stem::Drums)),
        (Part::Bass, other || has(Stem::Bass)),
        (Part::Vocals, other || has(Stem::Vocals)),
    ];
    let count = need.iter().filter(|(_, n)| *n).count().max(1);
    let mut made: [Option<[Vec<f32>; 2]>; 3] = [None, None, None];
    let mut done = 0;
    for (slot, (part, needed)) in made.iter_mut().zip(need) {
        if !needed {
            continue;
        }
        let masked = masked(tools, &spec, &mask, part);
        let mut out = tools.stft.synthesise(&masked, len);
        if tools.mono {
            // The two channels are the same signal: make them the same bits.
            out[1] = out[0].clone();
        }
        *slot = Some(out);
        control.check()?;
        done += 1;
        progress(0.55 + 0.45 * done as f32 / count as f32);
    }
    let [drums, bass, vocals] = made;
    let rest = if other {
        let mut rest = [l, r];
        for stem in [&drums, &bass, &vocals].into_iter().flatten() {
            for (x, s) in rest.iter_mut().zip(stem) {
                for (v, d) in x.iter_mut().zip(s) {
                    *v -= d;
                }
            }
        }
        if tools.mono {
            rest[1] = rest[0].clone();
        }
        Some(rest)
    } else {
        None
    };
    let mut drums = drums;
    let mut bass = bass;
    let mut vocals = vocals;
    let mut rest = rest;
    Ok(wanted
        .iter()
        .map(|stem| {
            let taken = match stem {
                Stem::Drums => drums.take(),
                Stem::Bass => bass.take(),
                Stem::Vocals => vocals.take(),
                Stem::Other => rest.take(),
            };
            // A stem asked for twice gets silence the second time: callers ask once.
            taken.unwrap_or_else(|| [vec![0f32; len], vec![0f32; len]])
        })
        .collect())
}

/// The soft mask of the percussive part for every cell, `frames * bins`, `0..=1`.
fn percussive_mask(spec: &Spectrogram, bins: usize, control: &Control<'_>) -> Vec<f32> {
    let frames = spec.frames;
    let mut mag = vec![0f32; frames * bins];
    for f in 0..frames {
        let base = f * 2 * bins;
        for k in 0..bins {
            let (lr, li) = (spec.re[base + k], spec.im[base + k]);
            let (rr, ri) = (spec.re[base + bins + k], spec.im[base + bins + k]);
            mag[f * bins + k] = (0.5 * (lr * lr + li * li + rr * rr + ri * ri)).sqrt();
        }
    }
    let mut mask = vec![0f32; frames * bins];
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .clamp(1, MAX_THREADS);
    let rows = frames.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (n, out) in mask.chunks_mut(rows * bins).enumerate() {
            let mag = &mag;
            s.spawn(move || {
                let first = n * rows;
                let mut window = [0f32; 17];
                for (r, row) in out.chunks_mut(bins).enumerate() {
                    if control.cancelled() {
                        return;
                    }
                    let f = first + r;
                    for (k, cell) in row.iter_mut().enumerate() {
                        // Across time: what lasts.
                        for (d, w) in window[..TIME_MEDIAN].iter_mut().enumerate() {
                            let g = (f + d).saturating_sub(TIME_MEDIAN / 2).min(frames - 1);
                            *w = mag[g * bins + k];
                        }
                        let h = median(&mut window[..TIME_MEDIAN]);
                        // Across frequency: what is broadband.
                        for (d, w) in window[..FREQ_MEDIAN].iter_mut().enumerate() {
                            let j = (k + d).saturating_sub(FREQ_MEDIAN / 2).min(bins - 1);
                            *w = mag[f * bins + j];
                        }
                        let p = median(&mut window[..FREQ_MEDIAN]);
                        let (h2, p2) = (h * h, p * p);
                        *cell = if h2 + p2 > 1e-20 { p2 / (h2 + p2) } else { 0.0 };
                    }
                }
            });
        }
    });
    mask
}

/// The middle value (the length is odd).
fn median(x: &mut [f32]) -> f32 {
    let mid = x.len() / 2;
    *x.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// The spectrogram of one filtered stem: the mix's cells times the part's gain.
fn masked(tools: &Tools, spec: &Spectrogram, mask: &[f32], part: Part) -> Spectrogram {
    let bins = tools.stft.bins();
    let mut out = Spectrogram {
        frames: spec.frames,
        re: vec![0.0; spec.re.len()],
        im: vec![0.0; spec.im.len()],
    };
    for f in 0..spec.frames {
        let base = f * 2 * bins;
        for k in 0..bins {
            let percussive = mask[f * bins + k];
            let gain = match part {
                Part::Drums => percussive,
                Part::Bass => (1.0 - percussive) * tools.bass[k],
                Part::Vocals => {
                    (1.0 - percussive)
                        * tools.voice[k]
                        * if tools.mono {
                            1.0
                        } else {
                            centred(spec, base, bins, k)
                        }
                }
            };
            for c in 0..2 {
                let i = base + c * bins + k;
                out.re[i] = spec.re[i] * gain;
                out.im[i] = spec.im[i] * gain;
            }
        }
    }
    out
}

/// How centred the cell is, `0..=1`: one when left and right are the same, zero when they are
/// out of phase or only one side has anything.
fn centred(spec: &Spectrogram, base: usize, bins: usize, k: usize) -> f32 {
    let (lr, li) = (spec.re[base + k], spec.im[base + k]);
    let (rr, ri) = (spec.re[base + bins + k], spec.im[base + bins + k]);
    let power = lr * lr + li * li + rr * rr + ri * ri;
    if power < 1e-20 {
        return 0.0;
    }
    let similar = (2.0 * (lr * rr + li * ri) / power).clamp(0.0, 1.0);
    similar * similar
}
