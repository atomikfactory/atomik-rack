//! The tonal engine: a phase vocoder with identity phase locking.
//!
//! * **Frames.** `N` = 2048 samples at 44.1 and 48 kHz (twice that above 60 kHz,
//!   [`frame_for_rate`]), Hann window for the analysis and again for the synthesis, a synthesis
//!   hop of `N / 4` (so the squared windows add up to a constant `1.5`, divided out). Output
//!   frame `k` starts at output frame `(k - 3) N / 4`; its analysis frame is centred on the
//!   source frame the time map gives for the output frame at its centre.
//! * **Phase.** Each analysis frame is paired with the frame `N / 4` samples before it; the
//!   phase difference of a bin over that lag, less what the bin centre advances, is the
//!   deviation from the bin centre and so gives the instantaneous frequency. A spectral peak (a
//!   bin that is at least as large as its two neighbours on each side) advances its synthesis
//!   phase by `N / 4` samples of that frequency; every other bin takes the phase rotation of
//!   its nearest peak, which keeps the relation between a partial and its skirt (identity phase
//!   locking), so a chord does not smear.
//! * **Stereo.** The two channels share the transforms (the analysis frame and its lagged twin
//!   share one complex transform per channel, both channels share the inverse), each channel
//!   with its own peaks and phases. With **linked phases** the peaks and the phase rotations
//!   come from the mid signal (from the louder channel for a frame whose mid is nearly empty)
//!   and both channels take the same rotation, so every bin keeps the phase relation between
//!   left and right and a stretched stereo clip keeps its image.
//! * **Formants.** With an envelope scale other than 1 each frame's bins are multiplied by the
//!   gain of [`formant::gains`] (the envelope of the analysis frame read at `scale` times the
//!   bin, over the envelope itself) before the synthesis, see [`super::formant`].
//! * Everything is `f64` with the deterministic trigonometry, so the result is the same on
//!   every platform.

use super::fft::{Fft, atan2_det, sin_cos_det};
use super::formant::{self, Envelope};
use super::map::TimeMap;
use crate::det_trig::cos_cycles;
use crate::frame_for_rate;

/// Overlap-add gain of four Hann-squared windows.
const OLA_GAIN: f64 = 1.5;

/// What the tonal engine does besides the stretch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TonalOptions {
    /// 1 leaves the spectral envelope alone; otherwise bin `j` takes the envelope of bin
    /// `j * env_scale` (see [`formant::gains`]).
    pub(crate) env_scale: f64,
    /// Both channels take the phase rotation of the mid (louder) channel.
    pub(crate) link_stereo: bool,
}

struct Channel<'a> {
    src: &'a [f32],
    /// The synthesised spectrum of the previous frame (bins `0..=N/2`).
    prev_re: Vec<f64>,
    prev_im: Vec<f64>,
    out: Vec<f32>,
}

/// The spectrum of an analysis frame (`a`) and of its lagged twin (`b`), bins `0..=N/2`.
struct Analysis {
    ar: Vec<f64>,
    ai: Vec<f64>,
    br: Vec<f64>,
    bi: Vec<f64>,
}

impl Analysis {
    fn new(half: usize) -> Self {
        Self {
            ar: vec![0.0; half + 1],
            ai: vec![0.0; half + 1],
            br: vec![0.0; half + 1],
            bi: vec![0.0; half + 1],
        }
    }

    fn energy(&self) -> f64 {
        self.ar
            .iter()
            .zip(&self.ai)
            .map(|(r, i)| r * r + i * i)
            .sum()
    }
}

/// Scratch of the phase work of one frame.
struct Locker {
    mag2: Vec<f64>,
    peaks: Vec<usize>,
    rot: Vec<(f64, f64)>,
}

impl Locker {
    /// The phase rotation of every spectral peak of `a`, advanced from the previous synthesised
    /// frame `prev` (frame `k`).
    fn peaks_and_rotations(
        &mut self,
        a: &Analysis,
        prev: (&[f64], &[f64]),
        k: i64,
        (n, hs, half): (usize, usize, usize),
    ) {
        let (ar, ai, br, bi) = (&a.ar, &a.ai, &a.br, &a.bi);
        for j in 0..=half {
            self.mag2[j] = ar[j] * ar[j] + ai[j] * ai[j];
        }
        let mag2 = &self.mag2;
        self.peaks.clear();
        for j in 0..=half {
            let m = mag2[j];
            let left = (1..=2).all(|d| j < d || m >= mag2[j - d]);
            let right = (1..=2).all(|d| j + d > half || m > mag2[j + d]);
            if m > 0.0 && left && right {
                self.peaks.push(j);
            }
        }
        self.rot.clear();
        for &p in &self.peaks {
            if k == 0 {
                self.rot.push((1.0, 0.0));
                continue;
            }
            // Phase advance of the partial over one synthesis hop.
            let dphi = atan2_det(ai[p] * br[p] - ar[p] * bi[p], ar[p] * br[p] + ai[p] * bi[p]);
            let centre_advance = std::f64::consts::TAU * p as f64 * hs as f64 / n as f64;
            let alpha = centre_advance + princarg(dphi - centre_advance);
            let (s, co) = sin_cos_det(alpha);
            let xm = mag2[p].sqrt();
            let (xr, xi) = (ar[p] / xm, ai[p] / xm);
            let pm = (prev.0[p] * prev.0[p] + prev.1[p] * prev.1[p]).sqrt();
            // The previous synthesis phase of the bin; the analysis phase when it was empty.
            let (ur, ui) = if pm > 1.0e-12 {
                (prev.0[p] / pm, prev.1[p] / pm)
            } else {
                (xr, xi)
            };
            // rotation = u * e^(i alpha) * conj(x)
            let (vr, vi) = (ur * co - ui * s, ur * s + ui * co);
            self.rot.push((vr * xr + vi * xi, vi * xr - vr * xi));
        }
    }

    /// Rotates the bins of `a` into `yr`/`yi`: each takes the rotation of its nearest peak.
    fn spread(&self, a: &Analysis, half: usize, yr: &mut [f64], yi: &mut [f64]) {
        let peaks = &self.peaks;
        let mut region = 0usize;
        for j in 0..=half {
            while region + 1 < peaks.len()
                && peaks[region + 1].abs_diff(j) < peaks[region].abs_diff(j)
            {
                region += 1;
            }
            let (rr, ri) = self.rot.get(region).copied().unwrap_or((1.0, 0.0));
            yr[j] = a.ar[j] * rr - a.ai[j] * ri;
            yi[j] = a.ar[j] * ri + a.ai[j] * rr;
        }
        yi[0] = 0.0;
        yi[half] = 0.0;
    }
}

fn sample(src: &[f32], i: i64) -> f64 {
    usize::try_from(i)
        .ok()
        .and_then(|i| src.get(i))
        .map_or(0.0, |s| f64::from(*s))
}

/// Wraps `x` to `(-pi, pi]`.
fn princarg(x: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    x - tau * (x / tau).round()
}

pub(crate) fn tonal(
    source: &[&[f32]],
    map: &TimeMap,
    out_frames: usize,
    sample_rate: u32,
    options: TonalOptions,
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<Vec<f32>>> {
    let n = frame_for_rate(sample_rate as f32, 2048);
    let hs = n / 4;
    let half = n / 2;
    let fft = Fft::new(n);
    let window: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * cos_cycles(i as f64 / n as f64))
        .collect();
    let mut chans: Vec<Channel> = source
        .iter()
        .take(2)
        .map(|src| Channel {
            src,
            prev_re: vec![0.0; half + 1],
            prev_im: vec![0.0; half + 1],
            out: vec![0.0; out_frames],
        })
        .collect();
    if chans.is_empty() {
        return Some(Vec::new());
    }
    let link = options.link_stereo && chans.len() == 2;
    let shifted = options.env_scale.is_finite() && (options.env_scale - 1.0).abs() > 1.0e-9;
    let envelope = Envelope::new(n, sample_rate);
    let (mut zr, mut zi) = (vec![0.0f64; n], vec![0.0f64; n]);
    // Per channel spectra of this frame after the phase work.
    let mut spec: Vec<(Vec<f64>, Vec<f64>)> = chans
        .iter()
        .map(|_| (vec![0.0; half + 1], vec![0.0; half + 1]))
        .collect();
    let mut an: Vec<Analysis> = chans.iter().map(|_| Analysis::new(half)).collect();
    let mut mid = Analysis::new(half);
    let (mut prev_mr, mut prev_mi) = (vec![0.0f64; half + 1], vec![0.0f64; half + 1]);
    let mut locker = Locker {
        mag2: vec![0.0; half + 1],
        peaks: Vec::new(),
        rot: Vec::new(),
    };
    let (mut mag, mut log_env, mut gain) = (
        vec![0.0f64; half + 1],
        vec![0.0f64; half + 1],
        vec![0.0f64; half + 1],
    );
    let dims = (n, hs, half);
    let hop = hs as i64;
    let mut k = 0i64;
    loop {
        let q = (k - 3) * hop;
        if q >= out_frames as i64 {
            break;
        }
        if (k as u64).is_multiple_of(16) && cancel() {
            return None;
        }
        let centre = q + half as i64;
        let a0 = map.src_at(centre as f64).round() as i64 - half as i64;
        let b0 = a0 - hop;
        for (c, a) in chans.iter().zip(an.iter_mut()) {
            // Analysis frame and its lagged twin in one transform.
            for i in 0..n {
                zr[i] = sample(c.src, a0 + i as i64) * window[i];
                zi[i] = sample(c.src, b0 + i as i64) * window[i];
            }
            fft.run(&mut zr, &mut zi, false);
            for j in 0..=half {
                let jn = (n - j) % n;
                a.ar[j] = 0.5 * (zr[j] + zr[jn]);
                a.ai[j] = 0.5 * (zi[j] - zi[jn]);
                a.br[j] = 0.5 * (zi[j] + zi[jn]);
                a.bi[j] = -0.5 * (zr[j] - zr[jn]);
            }
        }
        if link {
            // The reference of both channels: the mid, or the louder channel when the two are
            // nearly in opposite phase and the mid says little.
            for j in 0..=half {
                mid.ar[j] = 0.5 * (an[0].ar[j] + an[1].ar[j]);
                mid.ai[j] = 0.5 * (an[0].ai[j] + an[1].ai[j]);
                mid.br[j] = 0.5 * (an[0].br[j] + an[1].br[j]);
                mid.bi[j] = 0.5 * (an[0].bi[j] + an[1].bi[j]);
            }
            let (el, er) = (an[0].energy(), an[1].energy());
            let louder = usize::from(er > el);
            let use_mid = mid.energy() * 4.0 >= el.max(er) * 0.01;
            for j in 0..=half {
                if use_mid {
                    prev_mr[j] = 0.5 * (chans[0].prev_re[j] + chans[1].prev_re[j]);
                    prev_mi[j] = 0.5 * (chans[0].prev_im[j] + chans[1].prev_im[j]);
                } else {
                    prev_mr[j] = chans[louder].prev_re[j];
                    prev_mi[j] = chans[louder].prev_im[j];
                }
            }
            let reference = if use_mid { &mid } else { &an[louder] };
            locker.peaks_and_rotations(reference, (&prev_mr, &prev_mi), k, dims);
            for ((c, a), (yr, yi)) in chans.iter_mut().zip(&an).zip(spec.iter_mut()) {
                locker.spread(a, half, yr, yi);
                c.prev_re.copy_from_slice(yr);
                c.prev_im.copy_from_slice(yi);
            }
        } else {
            for ((c, a), (yr, yi)) in chans.iter_mut().zip(&an).zip(spec.iter_mut()) {
                locker.peaks_and_rotations(a, (&c.prev_re, &c.prev_im), k, dims);
                locker.spread(a, half, yr, yi);
                c.prev_re.copy_from_slice(yr);
                c.prev_im.copy_from_slice(yi);
            }
        }
        if shifted {
            for (a, (yr, yi)) in an.iter().zip(spec.iter_mut()) {
                for (m, (re, im)) in mag.iter_mut().zip(a.ar.iter().zip(&a.ai)) {
                    *m = (re * re + im * im).sqrt();
                }
                envelope.log_envelope(&fft, &mag, &mut zr, &mut zi, &mut log_env);
                formant::gains(&log_env, options.env_scale, &mut gain);
                for j in 0..=half {
                    yr[j] *= gain[j];
                    yi[j] *= gain[j];
                }
            }
        }
        // Synthesis: both channels in one inverse transform.
        let has_r = spec.len() > 1;
        for j in 0..=half {
            let (lr, li) = (spec[0].0[j], spec[0].1[j]);
            let (rr, ri) = if has_r {
                (spec[1].0[j], spec[1].1[j])
            } else {
                (0.0, 0.0)
            };
            zr[j] = lr - ri;
            zi[j] = li + rr;
            if j > 0 && j < half {
                zr[n - j] = lr + ri;
                zi[n - j] = -li + rr;
            }
        }
        fft.run(&mut zr, &mut zi, true);
        for i in 0..n {
            let Ok(at) = usize::try_from(q + i as i64) else {
                continue;
            };
            if at >= out_frames {
                break;
            }
            let g = window[i] / OLA_GAIN;
            chans[0].out[at] += (zr[i] * g) as f32;
            if has_r {
                chans[1].out[at] += (zi[i] * g) as f32;
            }
        }
        k += 1;
    }
    Some(chans.into_iter().map(|c| c.out).collect())
}
