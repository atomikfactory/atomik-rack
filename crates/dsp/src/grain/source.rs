//! The built-in source of the granular sampler: a sung vowel pad, made of arithmetic.
//!
//! Three seconds of a voice-like tone at middle C: 22 harmonics of a 261.6 Hz fundamental with a
//! 0.4 % vibrato at 5.2 Hz, shaped by three formant peaks that glide through four vowels (ah, oh,
//! eh, ee, each held for a fifth of its stretch) so the position in the sample is a timbre
//! control, and a little breath noise. The harmonics come from one phase by the Chebyshev
//! recurrence `sin(h x) = 2 cos(x) sin((h - 1) x) - sin((h - 2) x)`; the noise is a seeded
//! [`Rng`] through a one-pole low-pass. Only IEEE `+ - * /` and this crate's polynomial sine are
//! used, so the buffer is bit-identical on every platform.

use crate::Rng;
use crate::det_trig::{cos_cycles, sin_cycles};

/// Length of the source in seconds.
pub const DEFAULT_SOURCE_SECONDS: f32 = 3.0;
/// The key at which the source plays at its recorded pitch (middle C).
pub const DEFAULT_SOURCE_ROOT: u8 = 60;

/// Fundamental of the source (key 60).
const F0_HZ: f64 = 261.625_565_300_598_6;
/// Vibrato depth (fraction of the fundamental) and rate.
const VIBRATO: f64 = 0.004;
const VIBRATO_HZ: f64 = 5.2;
/// Highest harmonic frequency.
const TOP_HZ: f64 = 6_000.0;
/// Samples per block over which the harmonic amplitudes are held.
const BLOCK: usize = 16;
/// Peak level of the finished buffer.
const PEAK: f32 = 0.7;
/// Fade at both ends, in seconds.
const EDGE_S: f64 = 0.03;
/// Seed of the breath noise.
const NOISE_SEED: u64 = 0xA70F_0CA1;

/// A formant: centre and bandwidth in Hz.
type Formant = (f64, f64);

/// The vowels: ah, oh, eh, ee.
const VOWELS: [[Formant; 3]; 4] = [
    [(800.0, 90.0), (1150.0, 100.0), (2800.0, 120.0)],
    [(450.0, 80.0), (800.0, 90.0), (2830.0, 120.0)],
    [(400.0, 80.0), (1700.0, 100.0), (2600.0, 120.0)],
    [(300.0, 70.0), (2300.0, 100.0), (3000.0, 120.0)],
];

/// The three formants at time `u` in `0..=1` of the source.
fn formants(u: f64) -> [Formant; 3] {
    let x = u.clamp(0.0, 1.0) * (VOWELS.len() - 1) as f64;
    let i = (x.floor() as usize).min(VOWELS.len() - 2);
    // Hold the vowel for the first fifth and the last fifth of its stretch, glide between.
    let t = ((x - i as f64 - 0.2) / 0.6).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t);
    std::array::from_fn(|k| {
        let (a, b) = (VOWELS[i][k], VOWELS[i + 1][k]);
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    })
}

/// The mono source at `sample_rate` Hz: [`DEFAULT_SOURCE_SECONDS`] long, peaking at 0.7, playing
/// at its recorded pitch on key [`DEFAULT_SOURCE_ROOT`]. A non-finite or tiny rate is limited to
/// 8 kHz.
pub fn default_source(sample_rate: f32) -> Vec<f32> {
    let sr = if sample_rate.is_finite() {
        f64::from(sample_rate).max(8_000.0)
    } else {
        8_000.0
    };
    let frames = (f64::from(DEFAULT_SOURCE_SECONDS) * sr).round() as usize;
    let harmonics = ((TOP_HZ.min(0.4 * sr)) / F0_HZ).floor().max(1.0) as usize;
    let mut out = vec![0.0f32; frames];
    let mut amp = vec![0.0f64; harmonics + 1];
    let mut phase = 0.0f64;
    let mut vib = 0.0f64;
    let mut noise = Rng::new(NOISE_SEED);
    let mut lp = 0.0f64;
    for (b, block) in out.chunks_mut(BLOCK).enumerate() {
        let u = (b * BLOCK) as f64 / frames as f64;
        let f = formants(u);
        for (h, a) in amp.iter_mut().enumerate().skip(1) {
            let hz = h as f64 * F0_HZ;
            let gain: f64 = f
                .iter()
                .map(|&(c, w)| {
                    let d = (hz - c) / w;
                    1.0 / (1.0 + d * d)
                })
                .sum();
            *a = (0.04 + gain) / h as f64;
        }
        for s in block.iter_mut() {
            let (s1, c1) = (sin_cycles(phase), cos_cycles(phase));
            let (mut prev, mut cur) = (0.0f64, s1);
            let mut y = amp[1] * s1;
            for a in &amp[2..] {
                let next = 2.0 * c1 * cur - prev;
                y += a * next;
                prev = cur;
                cur = next;
            }
            // Breath: low-passed white noise.
            lp += 0.3 * (f64::from(noise.next_bipolar()) - lp);
            *s = (y + 0.25 * lp) as f32;
            vib += VIBRATO_HZ / sr;
            phase += F0_HZ * (1.0 + VIBRATO * sin_cycles(vib)) / sr;
            phase -= phase.floor();
        }
    }
    let edge = (EDGE_S * sr) as usize;
    for i in 0..edge.min(frames / 2) {
        let g = (0.5 - 0.5 * cos_cycles(0.5 * i as f64 / edge as f64)) as f32;
        out[i] *= g;
        out[frames - 1 - i] *= g;
    }
    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if peak > 0.0 {
        let k = PEAK / peak;
        out.iter_mut().for_each(|x| *x *= k);
    }
    out
}
