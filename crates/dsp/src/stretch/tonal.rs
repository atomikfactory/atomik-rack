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
//!   with its own peaks and phases.
//! * Everything is `f64` with the deterministic trigonometry, so the result is the same on
//!   every platform.

use super::fft::{Fft, atan2_det, sin_cos_det};
use super::map::TimeMap;
use crate::det_trig::cos_cycles;
use crate::frame_for_rate;

/// Overlap-add gain of four Hann-squared windows.
const OLA_GAIN: f64 = 1.5;

struct Channel<'a> {
    src: &'a [f32],
    /// The synthesised spectrum of the previous frame (bins `0..=N/2`).
    prev_re: Vec<f64>,
    prev_im: Vec<f64>,
    out: Vec<f32>,
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
    let (mut zr, mut zi) = (vec![0.0f64; n], vec![0.0f64; n]);
    // Per channel spectra of this frame after the phase work.
    let mut spec: Vec<(Vec<f64>, Vec<f64>)> = chans
        .iter()
        .map(|_| (vec![0.0; half + 1], vec![0.0; half + 1]))
        .collect();
    let (mut ar, mut ai) = (vec![0.0f64; half + 1], vec![0.0f64; half + 1]);
    let (mut br, mut bi) = (vec![0.0f64; half + 1], vec![0.0f64; half + 1]);
    let mut mag2 = vec![0.0f64; half + 1];
    let mut peaks: Vec<usize> = Vec::new();
    let mut rot: Vec<(f64, f64)> = Vec::new();
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
        for (c, (yr, yi)) in chans.iter_mut().zip(spec.iter_mut()) {
            // Analysis frame and its lagged twin in one transform.
            for i in 0..n {
                zr[i] = sample(c.src, a0 + i as i64) * window[i];
                zi[i] = sample(c.src, b0 + i as i64) * window[i];
            }
            fft.run(&mut zr, &mut zi, false);
            for j in 0..=half {
                let jn = (n - j) % n;
                ar[j] = 0.5 * (zr[j] + zr[jn]);
                ai[j] = 0.5 * (zi[j] - zi[jn]);
                br[j] = 0.5 * (zi[j] + zi[jn]);
                bi[j] = -0.5 * (zr[j] - zr[jn]);
                mag2[j] = ar[j] * ar[j] + ai[j] * ai[j];
            }
            peaks.clear();
            for j in 0..=half {
                let m = mag2[j];
                let left = (1..=2).all(|d| j < d || m >= mag2[j - d]);
                let right = (1..=2).all(|d| j + d > half || m > mag2[j + d]);
                if m > 0.0 && left && right {
                    peaks.push(j);
                }
            }
            rot.clear();
            for &p in &peaks {
                if k == 0 {
                    rot.push((1.0, 0.0));
                    continue;
                }
                // Phase advance of the partial over one synthesis hop.
                let dphi = atan2_det(ai[p] * br[p] - ar[p] * bi[p], ar[p] * br[p] + ai[p] * bi[p]);
                let centre_advance = std::f64::consts::TAU * p as f64 * hs as f64 / n as f64;
                let alpha = centre_advance + princarg(dphi - centre_advance);
                let (s, co) = sin_cos_det(alpha);
                let xm = mag2[p].sqrt();
                let (xr, xi) = (ar[p] / xm, ai[p] / xm);
                let pm = (c.prev_re[p] * c.prev_re[p] + c.prev_im[p] * c.prev_im[p]).sqrt();
                // The previous synthesis phase of the bin; the analysis phase when it was empty.
                let (ur, ui) = if pm > 1.0e-12 {
                    (c.prev_re[p] / pm, c.prev_im[p] / pm)
                } else {
                    (xr, xi)
                };
                // rotation = u * e^(i alpha) * conj(x)
                let (vr, vi) = (ur * co - ui * s, ur * s + ui * co);
                rot.push((vr * xr + vi * xi, vi * xr - vr * xi));
            }
            let mut region = 0usize;
            for j in 0..=half {
                while region + 1 < peaks.len()
                    && peaks[region + 1].abs_diff(j) < peaks[region].abs_diff(j)
                {
                    region += 1;
                }
                let (rr, ri) = rot.get(region).copied().unwrap_or((1.0, 0.0));
                yr[j] = ar[j] * rr - ai[j] * ri;
                yi[j] = ar[j] * ri + ai[j] * rr;
            }
            yi[0] = 0.0;
            yi[half] = 0.0;
            c.prev_re.copy_from_slice(yr);
            c.prev_im.copy_from_slice(yi);
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
