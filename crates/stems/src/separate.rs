//! The offline separation: chunks with a crossfade, the four networks, ratio masks, the inverse
//! transform and the streaming of the finished samples to a [`StemSink`].
//!
//! Each chunk is about twenty seconds and is analysed with two seconds of context on each side,
//! so the recurrent layers have settled by the time the samples that are kept begin. The
//! estimates of the four networks give ratio masks (each stem's power over the sum of the four),
//! which are applied to the complex spectrogram of the mix: the four stems add up to the mix
//! wherever the networks hear anything. Neighbouring chunks are crossfaded linearly over the
//! second around their boundary.

use crate::net::Net;
use crate::stft::{Spectrogram, Stft};
use crate::torch;
use crate::{Control, Stem, StemError, files};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// The seconds of signal a chunk keeps (not counting its context).
const CORE_SECONDS: f64 = 20.0;
/// The seconds of context on each side of a chunk.
const CONTEXT_SECONDS: f64 = 2.0;

/// The receiver of the finished stems. For every stem the pieces come in order and cover the
/// song once, without gaps.
pub trait StemSink {
    /// The next samples of `stem`, left and right.
    fn write(&mut self, stem: Stem, left: &[f32], right: &[f32]) -> Result<(), StemError>;
}

/// The four networks and the transform they share.
#[derive(Debug, Clone)]
pub struct Models {
    nets: Vec<Net>,
    stft: Stft,
    rate: u32,
}

impl Models {
    /// Loads the four weight files from `dir` (the names of [`files::MODEL_FILES`]). The caller
    /// has checked them against the pinned SHA-256 (the model store does).
    pub fn load(dir: &Path) -> Result<Self, StemError> {
        let mut nets = Vec::new();
        for f in files::MODEL_FILES {
            let path = dir.join(f.name);
            let bytes =
                std::fs::read(&path).map_err(|e| StemError::Weights(format!("{}: {e}", f.name)))?;
            let state = torch::read_state(&bytes)
                .map_err(|e| StemError::Weights(format!("{}: {e}", f.name)))?;
            nets.push(Net::from_state(&state)?);
        }
        Self::from_nets(nets)
    }

    /// The models from four networks in the order of [`Stem::ALL`].
    pub fn from_nets(nets: Vec<Net>) -> Result<Self, StemError> {
        if nets.len() != 4 {
            return Err(StemError::Weights("four networks are needed".into()));
        }
        let first = &nets[0];
        if nets
            .iter()
            .any(|n| n.n_fft != first.n_fft || n.rate != first.rate || n.window != first.window)
        {
            return Err(StemError::Weights("the networks do not agree".into()));
        }
        Ok(Self {
            stft: Stft::new(first.n_fft, first.window.clone()),
            rate: first.rate,
            nets,
        })
    }

    /// The sample rate the audio must have.
    pub fn rate(&self) -> u32 {
        self.rate
    }
}

/// A piece of the song being assembled: the output of one stem and channel between the samples
/// already sent on and the end of the last chunk.
struct Tail {
    /// The first song sample that is held.
    from: usize,
    data: [Vec<f32>; 2],
}

/// Separates a stereo song at the models' rate into stems.
///
/// `wanted` are the stems sent to the sink (all four networks run anyway: the masks need every
/// estimate). `progress` is called with `0..=1` as chunks finish; `cancel` stops the run (the
/// result is then `Err(Cancelled)` and the sink has been given a prefix only). Non-finite
/// samples are treated as silence.
pub fn separate(
    models: &Models,
    left: &[f32],
    right: &[f32],
    wanted: &[Stem],
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
    let rate = f64::from(models.rate);
    let core = (CORE_SECONDS * rate) as usize;
    let context = (CONTEXT_SECONDS * rate) as usize;
    let half = context / 2;
    let chunks = len.div_ceil(core).max(1);
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
        let pieces = chunk(models, &left[a..b], &right[a..b], wanted, &control)?;
        // Weights: a ramp over `half` samples either side of each inner boundary.
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

/// The wanted stems of one stretch of the song (with its context).
fn chunk(
    models: &Models,
    left: &[f32],
    right: &[f32],
    wanted: &[Stem],
    control: &Control<'_>,
) -> Result<Vec<[Vec<f32>; 2]>, StemError> {
    let len = left.len();
    let clean = |x: &[f32]| -> Vec<f32> {
        x.iter()
            .map(|v| if v.is_finite() { *v } else { 0.0 })
            .collect()
    };
    let (l, r) = (clean(left), clean(right));
    if l.iter().chain(&r).all(|v| *v == 0.0) {
        return Ok(wanted
            .iter()
            .map(|_| [vec![0f32; len], vec![0f32; len]])
            .collect());
    }
    let spec = models.stft.analyse(&l, &r);
    drop((l, r));
    let bins = models.stft.bins();
    let n = spec.frames * 2 * bins;
    let mag: Vec<f32> = (0..n).map(|j| spec.re[j].hypot(spec.im[j])).collect();
    // The four estimates, one thread each.
    let estimates: Vec<Result<Vec<f32>, StemError>> = std::thread::scope(|s| {
        let handles: Vec<_> = models
            .nets
            .iter()
            .map(|net| s.spawn(|| net.estimate(&mag, spec.frames, control)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or(Err(StemError::Cancelled)))
            .collect()
    });
    let mut est = Vec::with_capacity(4);
    for e in estimates {
        est.push(e?);
    }
    control.check()?;
    drop(mag);
    // Power sum over the four stems.
    let mut total = vec![0f32; n];
    for e in &est {
        for (t, v) in total.iter_mut().zip(e) {
            *t += v * v;
        }
    }
    let results: Vec<Result<[Vec<f32>; 2], StemError>> = std::thread::scope(|s| {
        let handles: Vec<_> = wanted
            .iter()
            .map(|stem| {
                let (e, total, spec) = (&est[stem.index()], &total, &spec);
                s.spawn(move || {
                    let mut masked = Spectrogram {
                        frames: spec.frames,
                        re: vec![0.0; n],
                        im: vec![0.0; n],
                    };
                    for j in 0..n {
                        let m = if total[j] > 1e-20 {
                            e[j] * e[j] / total[j]
                        } else {
                            0.0
                        };
                        masked.re[j] = spec.re[j] * m;
                        masked.im[j] = spec.im[j] * m;
                    }
                    Ok(models.stft.synthesise(&masked, len))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or(Err(StemError::Cancelled)))
            .collect()
    });
    results.into_iter().collect()
}
