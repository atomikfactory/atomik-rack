//! Short-time Fourier stage: windowed analysis, per-bin magnitude algorithms with the phase kept,
//! windowed overlap-add synthesis.
//!
//! * **Frame and hop:** a frame of `N` samples (a power of two; 1024 by default at 44.1 and
//!   48 kHz, see [`frame_for_rate`]) advances by `N/4`; analysis and synthesis both use a periodic
//!   Hann window, whose square sums to a constant `1.5` over four-fold overlap, so the
//!   overlap-add gain is normalised by `1/1.5` and an identity algorithm reconstructs the input
//!   exactly (to float precision).
//! * **Latency:** exactly `N` samples ([`Spectral::latency_frames`]): the output at sample `t` is
//!   the input at `t - N`; the first `N` outputs are zero.
//! * **Stereo:** the two channels share one complex FFT (left real, right imaginary) and are
//!   separated per bin. With `linked` (the default) every algorithm decides on the combined
//!   magnitude `sqrt((|L|^2 + |R|^2)/2)` and applies the same gain to both channels, so the stereo
//!   image is kept; unlinked, each channel decides alone.
//! * **Magnitudes** are normalised so that a full-scale sine centred on a bin reads 1.0 (0 dB).
//! * **Algorithms** ([`SpectralAlgo`]): identity; per-bin gate with hold; per-bin slew limiter;
//!   band-pass and band-stop in bin space; coarse level quantiser; and the combined `Lossy`
//!   (gate with hold, then level quantiser, then seeded dropouts of four-bin bands per frame).
//!   Gains are applied per frame (no smoothing between frames: the stepping is part of the
//!   sound).
//!
//! The FFT is an own radix-2 transform with twiddles from the deterministic sine of
//! `det_trig`, so results are bit-identical on every platform. All buffers are allocated in
//! [`Spectral::new`]; processing never allocates.

use crate::det_trig::{cos_cycles, sin_cycles};
use crate::rng::hash64;
use crate::{exp2_det, log2_det};

/// Smallest frame.
pub const MIN_FRAME: usize = 64;
/// Largest frame.
pub const MAX_FRAME: usize = 16_384;
/// Floor of the slew limiter's memory (about -80 dB).
const SLEW_FLOOR: f32 = 1.0e-4;
/// `log2(10)/20`.
const DB_TO_LOG2: f32 = 0.166_096_4;
/// Overlap-add gain of four Hann-squared windows.
const OLA_GAIN: f32 = 1.5;

/// The frame length for a sample rate: `base_frame` (rounded up to a power of two) up to 60 kHz,
/// doubled for every further doubling of the rate, so the frame spans a similar time at any rate.
pub fn frame_for_rate(sample_rate: f32, base_frame: usize) -> usize {
    let mut n = base_frame.next_power_of_two().clamp(MIN_FRAME, MAX_FRAME);
    let mut sr = sample_rate;
    while sr > 60_000.0 && n < MAX_FRAME {
        n *= 2;
        sr *= 0.5;
    }
    n
}

/// The per-bin algorithm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpectralAlgo {
    /// Pass-through (perfect reconstruction).
    Identity,
    /// Bins whose level is below `threshold_db` (0 dB is a full-scale sine) are attenuated by
    /// `(level / threshold)^2`, the lower the more; a bin that was at or above the threshold
    /// stays fully open for `hold_ms` after it falls below.
    Gate {
        /// Threshold in dB re a full-scale sine.
        threshold_db: f32,
        /// Hold time in ms.
        hold_ms: f32,
    },
    /// A bin's level may rise by at most `rise_db` per frame (falls pass at once): sudden
    /// onsets are smeared.
    Slew {
        /// Largest rise per frame in dB.
        rise_db: f32,
    },
    /// Keeps bins from `lo_hz` to `hi_hz` and removes the rest.
    BandPass {
        /// Lower edge in Hz.
        lo_hz: f32,
        /// Upper edge in Hz.
        hi_hz: f32,
    },
    /// Removes bins from `lo_hz` to `hi_hz` and keeps the rest.
    BandStop {
        /// Lower edge in Hz.
        lo_hz: f32,
        /// Upper edge in Hz.
        hi_hz: f32,
    },
    /// Rounds each bin's level (in dB) to a multiple of `step_db`.
    Quantise {
        /// Step in dB (clamped to `0.5..=24`).
        step_db: f32,
    },
    /// The poorly-coded sound in one pass: the [`SpectralAlgo::Gate`] decision, then the
    /// remaining level rounded to a multiple of `step_db` (as [`SpectralAlgo::Quantise`]), then
    /// a seeded dropout: every frame each band of four bins is silenced with probability
    /// `dropout` (the draw depends on the seed, the frame number and the band only).
    Lossy {
        /// Gate threshold in dB re a full-scale sine.
        threshold_db: f32,
        /// Gate hold in ms.
        hold_ms: f32,
        /// Quantiser step in dB (clamped to `0.5..=24`).
        step_db: f32,
        /// Probability `0..=1` that a band of four bins is dropped in a frame.
        dropout: f32,
    },
}

/// Radix-2 complex FFT with precomputed twiddles and bit reversal.
#[derive(Debug, Clone)]
struct Fft {
    n: usize,
    rev: Vec<u32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let bits = n.trailing_zeros();
        let rev = (0..n as u32)
            .map(|i| i.reverse_bits() >> (32 - bits))
            .collect();
        let cos = (0..n / 2)
            .map(|k| cos_cycles(k as f64 / n as f64) as f32)
            .collect();
        let sin = (0..n / 2)
            .map(|k| sin_cycles(k as f64 / n as f64) as f32)
            .collect();
        Self { n, rev, cos, sin }
    }

    /// In-place forward transform `X[k] = sum x[n] e^{-2 pi i k n / N}`.
    fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            let mut i = 0;
            while i < n {
                for j in 0..half {
                    let (wr, wi) = (self.cos[j * step], -self.sin[j * step]);
                    let (a, b) = (i + j, i + j + half);
                    let tr = wr * re[b] - wi * im[b];
                    let ti = wr * im[b] + wi * re[b];
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
                i += len;
            }
            len *= 2;
        }
    }

    /// In-place inverse transform including the `1/N` scale (swap trick).
    fn inverse(&self, re: &mut [f32], im: &mut [f32]) {
        self.forward(im, re);
        let s = 1.0 / self.n as f32;
        for v in re.iter_mut().chain(im.iter_mut()) {
            *v *= s;
        }
    }
}

/// Per-bin algorithm state of one decision channel.
#[derive(Debug, Clone)]
struct BinState {
    hold: Vec<u32>,
    prev: Vec<f32>,
    /// Seed of the dropout draws.
    seed: u64,
    /// Frames decided since the last clear (the dropout draws depend on it).
    frame: u32,
}

impl BinState {
    fn new(bins: usize, seed: u64) -> Self {
        Self {
            hold: vec![0; bins],
            prev: vec![SLEW_FLOOR; bins],
            seed,
            frame: 0,
        }
    }

    fn clear(&mut self) {
        self.hold.fill(0);
        self.prev.fill(SLEW_FLOOR);
        self.frame = 0;
    }
}

/// The gate decision of one bin (shared by `Gate` and `Lossy`).
#[inline]
fn gate_gain(m: f32, thr: f32, hold: u32, held: &mut u32) -> f32 {
    if m >= thr {
        *held = hold;
        1.0
    } else if *held > 0 {
        *held -= 1;
        1.0
    } else {
        let q = m / thr;
        q * q
    }
}

/// The quantiser gain of one bin of level `m` for a step in dB.
#[inline]
fn quantise_gain(m: f32, step: f32) -> f32 {
    if m < 1.0e-6 {
        1.0
    } else {
        let level = log2_det(m) / DB_TO_LOG2;
        let q = (level / step).round() * step;
        exp2_det((q - level) * DB_TO_LOG2)
    }
}

/// Computes the gain of every bin from its normalised magnitude.
fn decide(
    algo: SpectralAlgo,
    sample_rate: f32,
    frame: usize,
    hop: usize,
    mag: &[f32],
    state: &mut BinState,
    gain: &mut [f32],
) {
    let bin_hz = sample_rate / frame as f32;
    match algo {
        SpectralAlgo::Identity => gain.fill(1.0),
        SpectralAlgo::Gate {
            threshold_db,
            hold_ms,
        } => {
            let thr = exp2_det(threshold_db.clamp(-120.0, 0.0) * DB_TO_LOG2);
            let hold =
                (hold_ms.clamp(0.0, 10_000.0) * 0.001 * sample_rate / hop as f32).round() as u32;
            for (k, g) in gain.iter_mut().enumerate() {
                *g = gate_gain(mag[k], thr, hold, &mut state.hold[k]);
            }
        }
        SpectralAlgo::Slew { rise_db } => {
            let up = exp2_det(rise_db.clamp(0.0, 60.0) * DB_TO_LOG2);
            for (k, g) in gain.iter_mut().enumerate() {
                let m = mag[k];
                if m <= 0.0 {
                    state.prev[k] = (state.prev[k] * 0.5).max(SLEW_FLOOR);
                    *g = 1.0;
                } else {
                    let out = m.min(state.prev[k] * up);
                    state.prev[k] = out.max(SLEW_FLOOR);
                    *g = out / m;
                }
            }
        }
        SpectralAlgo::BandPass { lo_hz, hi_hz } => {
            for (k, g) in gain.iter_mut().enumerate() {
                let f = k as f32 * bin_hz;
                *g = if f >= lo_hz && f <= hi_hz { 1.0 } else { 0.0 };
            }
        }
        SpectralAlgo::BandStop { lo_hz, hi_hz } => {
            for (k, g) in gain.iter_mut().enumerate() {
                let f = k as f32 * bin_hz;
                *g = if f >= lo_hz && f <= hi_hz { 0.0 } else { 1.0 };
            }
        }
        SpectralAlgo::Quantise { step_db } => {
            let step = step_db.clamp(0.5, 24.0);
            for (k, g) in gain.iter_mut().enumerate() {
                *g = quantise_gain(mag[k], step);
            }
        }
        SpectralAlgo::Lossy {
            threshold_db,
            hold_ms,
            step_db,
            dropout,
        } => {
            let thr = exp2_det(threshold_db.clamp(-120.0, 0.0) * DB_TO_LOG2);
            let hold =
                (hold_ms.clamp(0.0, 10_000.0) * 0.001 * sample_rate / hop as f32).round() as u32;
            let step = step_db.clamp(0.5, 24.0);
            let p = if dropout.is_nan() {
                0.0
            } else {
                dropout.clamp(0.0, 1.0)
            };
            let mut dropped = false;
            for (k, g) in gain.iter_mut().enumerate() {
                if k % 4 == 0 {
                    let h = hash64(state.seed, u64::from(state.frame), (k / 4) as u64);
                    dropped = ((h >> 40) as f32) * (1.0 / 16_777_216.0) < p;
                }
                let gated = gate_gain(mag[k], thr, hold, &mut state.hold[k]);
                *g = if dropped {
                    0.0
                } else {
                    gated * quantise_gain(mag[k] * gated, step)
                };
            }
        }
    }
    state.frame = state.frame.wrapping_add(1);
}

/// Stereo STFT stage.
#[derive(Debug, Clone)]
pub struct Spectral {
    sample_rate: f32,
    n: usize,
    hop: usize,
    fft: Fft,
    window: Vec<f32>,
    ring: [Vec<f32>; 2],
    acc: [Vec<f32>; 2],
    re: Vec<f32>,
    im: Vec<f32>,
    spec: [Vec<f32>; 4],
    mag: [Vec<f32>; 2],
    gain: [Vec<f32>; 2],
    mag_scale: f32,
    t: usize,
    algo: SpectralAlgo,
    linked: bool,
    state: [BinState; 2],
}

impl Spectral {
    /// A stage with the given frame length (rounded up to a power of two within
    /// `64..=16384`), identity algorithm, linked stereo. Non-RT allocation.
    pub fn new(sample_rate: f32, frame: usize) -> Self {
        let n = frame.next_power_of_two().clamp(MIN_FRAME, MAX_FRAME);
        let bins = n / 2 + 1;
        let window = (0..n)
            .map(|i| {
                let s = sin_cycles(i as f64 / (2.0 * n as f64));
                (s * s) as f32
            })
            .collect();
        Self {
            sample_rate,
            n,
            hop: n / 4,
            fft: Fft::new(n),
            window,
            ring: [vec![0.0; n], vec![0.0; n]],
            acc: [vec![0.0; n], vec![0.0; n]],
            re: vec![0.0; n],
            im: vec![0.0; n],
            spec: std::array::from_fn(|_| vec![0.0; bins]),
            mag: [vec![0.0; bins], vec![0.0; bins]],
            gain: [vec![1.0; bins], vec![1.0; bins]],
            mag_scale: 4.0 / n as f32,
            t: 0,
            algo: SpectralAlgo::Identity,
            linked: true,
            state: [BinState::new(bins, 0), BinState::new(bins, 1)],
        }
    }

    /// Seeds the dropout draws of [`SpectralAlgo::Lossy`] (the two decision channels get
    /// different streams). A seed change takes effect from the next frame; `reset` restarts
    /// the frame count, so a render after `reset` equals the first one.
    pub fn set_seed(&mut self, seed: u64) {
        self.state[0].seed = hash64(seed, 0x5EC7, 0);
        self.state[1].seed = hash64(seed, 0x5EC7, 1);
    }

    /// The latency in samples (equal to the frame length).
    pub fn latency_frames(&self) -> u32 {
        self.n as u32
    }

    /// The frame length.
    pub fn frame(&self) -> usize {
        self.n
    }

    /// Selects the algorithm and its parameters. The per-bin state is cleared when the
    /// algorithm kind changes, not when only its parameters do.
    pub fn set_algorithm(&mut self, algo: SpectralAlgo) {
        if std::mem::discriminant(&algo) != std::mem::discriminant(&self.algo) {
            for s in &mut self.state {
                s.clear();
            }
        }
        self.algo = algo;
    }

    /// The current algorithm.
    pub fn algorithm(&self) -> SpectralAlgo {
        self.algo
    }

    /// Whether both channels share their decisions (the default).
    pub fn set_linked(&mut self, linked: bool) {
        self.linked = linked;
    }

    /// Clears the rings, the overlap-add buffers and the algorithm state.
    pub fn reset(&mut self) {
        for c in 0..2 {
            self.ring[c].fill(0.0);
            self.acc[c].fill(0.0);
            self.state[c].clear();
        }
        self.t = 0;
    }

    /// Processes one stereo sample; the output is the input of `latency_frames()` samples ago
    /// through the algorithm. Non-finite input counts as 0.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let slot = self.t % self.n;
        let out = (self.acc[0][slot], self.acc[1][slot]);
        self.acc[0][slot] = 0.0;
        self.acc[1][slot] = 0.0;
        self.ring[0][slot] = if l.is_finite() { l } else { 0.0 };
        self.ring[1][slot] = if r.is_finite() { r } else { 0.0 };
        self.t += 1;
        if self.t.is_multiple_of(self.hop) {
            self.frame_step();
        }
        if self.t >= 2 * self.n {
            self.t -= self.n; // keeps `t % n` and `t % hop` unchanged
        }
        out
    }

    /// Processes a stereo block in place (identical to per-sample calls).
    pub fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (x, y) = self.process(*a, *b);
            *a = x;
            *b = y;
        }
    }

    /// Analyses the newest `n` samples, applies the algorithm and overlap-adds the result.
    fn frame_step(&mut self) {
        let n = self.n;
        let bins = n / 2 + 1;
        let start = self.t % n; // the oldest sample of the frame
        for i in 0..n {
            let k = (start + i) % n;
            let w = self.window[i];
            self.re[i] = self.ring[0][k] * w;
            self.im[i] = self.ring[1][k] * w;
        }
        self.fft.forward(&mut self.re, &mut self.im);

        // Separate the two real channels from the packed transform:
        // X_L = (Z[k] + conj Z[N-k]) / 2, X_R = (Z[k] - conj Z[N-k]) / (2j).
        for k in 0..bins {
            let m = (n - k) % n;
            let (zr, zi) = (self.re[k], self.im[k]);
            let (cr, ci) = (self.re[m], -self.im[m]);
            self.spec[0][k] = 0.5 * (zr + cr);
            self.spec[1][k] = 0.5 * (zi + ci);
            self.spec[2][k] = 0.5 * (zi - ci);
            self.spec[3][k] = -0.5 * (zr - cr);
            for c in 0..2 {
                let (a, b) = (self.spec[2 * c][k], self.spec[2 * c + 1][k]);
                self.mag[c][k] = (a * a + b * b).sqrt() * self.mag_scale;
            }
        }

        // Decide.
        if self.linked {
            for k in 0..bins {
                let (a, b) = (self.mag[0][k], self.mag[1][k]);
                self.mag[0][k] = (0.5 * (a * a + b * b)).sqrt();
            }
            let [g0, g1] = &mut self.gain;
            let [s0, _] = &mut self.state;
            decide(
                self.algo,
                self.sample_rate,
                n,
                self.hop,
                &self.mag[0],
                s0,
                g0,
            );
            g1.copy_from_slice(g0);
        } else {
            for c in 0..2 {
                decide(
                    self.algo,
                    self.sample_rate,
                    n,
                    self.hop,
                    &self.mag[c],
                    &mut self.state[c],
                    &mut self.gain[c],
                );
            }
        }

        // Apply and repack: Z' = Y_L + j Y_R, Z'[N-k] = conj Y_L + j conj Y_R.
        for k in 0..bins {
            let (gl, gr) = (self.gain[0][k], self.gain[1][k]);
            let (lr, li) = (self.spec[0][k] * gl, self.spec[1][k] * gl);
            let (rr, ri) = (self.spec[2][k] * gr, self.spec[3][k] * gr);
            self.re[k] = lr - ri;
            self.im[k] = li + rr;
            if k != 0 && k != n / 2 {
                self.re[n - k] = lr + ri;
                self.im[n - k] = rr - li;
            }
        }
        self.fft.inverse(&mut self.re, &mut self.im);
        for i in 0..n {
            let k = (start + i) % n;
            let w = self.window[i] / OLA_GAIN;
            self.acc[0][k] += self.re[i] * w;
            self.acc[1][k] += self.im[i] * w;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{
        assert_chunk_invariant_stereo, db, noise, rms, sine, spectrum, tone_amplitude,
    };

    const SR: f32 = 48_000.0;
    const N: usize = 1024;

    fn run(s: &mut Spectral, l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut a, mut b) = (l.to_vec(), r.to_vec());
        s.process_block(&mut a, &mut b);
        (a, b)
    }

    fn err_db(out: &[f32], reference: &[f32]) -> f64 {
        let e: f64 = out
            .iter()
            .zip(reference)
            .map(|(a, b)| f64::from(*a - *b).powi(2))
            .sum();
        let s: f64 = reference.iter().map(|v| f64::from(*v).powi(2)).sum();
        10.0 * (e / s).log10()
    }

    #[test]
    fn fft_matches_the_definition_and_inverts() {
        let n = 64;
        let f = Fft::new(n);
        let x = noise(1, n, 1.0);
        let y = noise(2, n, 1.0);
        let (mut re, mut im) = (x.clone(), y.clone());
        f.forward(&mut re, &mut im);
        for k in [0usize, 1, 5, 17, 32, 63] {
            let (mut sr, mut si) = (0.0f64, 0.0f64);
            for t in 0..n {
                let w = -std::f64::consts::TAU * (k * t) as f64 / n as f64;
                sr += f64::from(x[t]) * w.cos() - f64::from(y[t]) * w.sin();
                si += f64::from(x[t]) * w.sin() + f64::from(y[t]) * w.cos();
            }
            assert!((f64::from(re[k]) - sr).abs() < 1e-4 && (f64::from(im[k]) - si).abs() < 1e-4);
        }
        f.inverse(&mut re, &mut im);
        for t in 0..n {
            assert!((re[t] - x[t]).abs() < 1e-5 && (im[t] - y[t]).abs() < 1e-5);
        }
    }

    #[test]
    fn identity_reconstructs_the_input_after_exactly_the_latency() {
        for frame in [256usize, 1024, 2048] {
            let mut s = Spectral::new(SR, frame);
            let x = noise(3, 20_000, 0.8);
            let y = noise(4, 20_000, 0.8);
            let (ol, or) = run(&mut s, &x, &y);
            let lat = s.latency_frames() as usize;
            assert_eq!(lat, frame);
            assert!(
                ol[..lat].iter().all(|v| v.abs() < 1e-6),
                "silent before the latency"
            );
            assert!(
                err_db(&ol[lat..], &x[..x.len() - lat]) < -90.0,
                "left {frame}"
            );
            assert!(
                err_db(&or[lat..], &y[..y.len() - lat]) < -90.0,
                "right {frame}"
            );
        }
    }

    #[test]
    fn latency_is_exact_for_an_impulse() {
        let mut s = Spectral::new(SR, N);
        let mut x = vec![0.0f32; 5_000];
        x[100] = 1.0;
        let (ol, _) = run(&mut s, &x, &vec![0.0; 5_000]);
        let peak = ol
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap();
        assert_eq!(peak.0, 100 + N);
        assert!((peak.1 - 1.0).abs() < 1e-5);
        assert!(
            ol.iter()
                .enumerate()
                .all(|(i, v)| i == 100 + N || v.abs() < 1e-5)
        );
    }

    #[test]
    fn frame_scales_with_the_sample_rate() {
        assert_eq!(frame_for_rate(44_100.0, 1024), 1024);
        assert_eq!(frame_for_rate(48_000.0, 1024), 1024);
        assert_eq!(frame_for_rate(88_200.0, 1024), 2048);
        assert_eq!(frame_for_rate(96_000.0, 1024), 2048);
        assert_eq!(frame_for_rate(192_000.0, 1024), 4096);
        assert_eq!(frame_for_rate(48_000.0, 512), 512);
        let s = Spectral::new(SR, 300);
        assert_eq!(s.frame(), 512);
        assert_eq!(Spectral::new(SR, 1).frame(), MIN_FRAME);
    }

    /// Output amplitude at `f` for input of two tones, windowed over `[from, to)` of the output.
    fn amp(y: &[f32], f: f64, from: usize, to: usize) -> f64 {
        tone_amplitude(&y[from..to], f, f64::from(SR))
    }

    #[test]
    fn gate_attenuates_quiet_bins_by_the_square_law_and_keeps_loud_ones() {
        let bin = f64::from(SR) / N as f64;
        let (fa, fb) = (40.0 * bin, 90.0 * bin);
        let n = 32_768;
        let a = sine(fa, f64::from(SR), n, 0.5);
        let b = sine(fb, f64::from(SR), n, 0.001);
        let x: Vec<f32> = a.iter().zip(&b).map(|(p, q)| p + q).collect();
        let mut s = Spectral::new(SR, N);
        s.set_algorithm(SpectralAlgo::Gate {
            threshold_db: -40.0,
            hold_ms: 0.0,
        });
        let (y, _) = run(&mut s, &x, &x);
        let (from, to) = (16_384, 32_768); // whole bins: whole cycles of both tones
        // Loud tone untouched.
        assert!(db(amp(&y, fa, from, to) / 0.5).abs() < 0.1);
        // Quiet tone: peak bin 0.001 against threshold 0.01 gives (0.1)^2 = -40 dB.
        let quiet = db(amp(&y, fb, from, to) / 0.001);
        assert!(quiet < -30.0 && quiet > -55.0, "{quiet}");
        // Without the gate it passes untouched.
        s.set_algorithm(SpectralAlgo::Identity);
        s.reset();
        let (y, _) = run(&mut s, &x, &x);
        assert!(db(amp(&y, fb, from, to) / 0.001).abs() < 0.5);
    }

    #[test]
    fn gate_holds_a_bin_open_for_the_programmed_time() {
        let bin = f64::from(SR) / N as f64;
        let f = 64.0 * bin;
        let drop_at = 12_000usize;
        let n = 48_000usize;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let a = if i < drop_at { 0.2 } else { 0.001 };
                a * (std::f64::consts::TAU * f * i as f64 / f64::from(SR)).sin() as f32
            })
            .collect();
        let measure = |hold_ms: f32| {
            let mut s = Spectral::new(SR, N);
            s.set_algorithm(SpectralAlgo::Gate {
                threshold_db: -40.0,
                hold_ms,
            });
            let (y, _) = run(&mut s, &x, &x);
            // Output time t shows input time t - N. Input windows after the drop:
            let at = |from_ms: f32, to_ms: f32| {
                let a = drop_at + N + (from_ms * 48.0) as usize;
                let b = drop_at + N + (to_ms * 48.0) as usize;
                amp(&y, f, a, b)
            };
            (at(40.0, 80.0), at(180.0, 300.0))
        };
        // 100 ms hold: still unattenuated 40..80 ms after the drop, gone after 180 ms.
        let (early, late) = measure(100.0);
        assert!(db(early / 0.001) > -3.0, "held: {early}");
        assert!(db(late / 0.001) < -25.0, "released: {late}");
        // No hold: already attenuated 40..80 ms after the drop.
        let (early0, _) = measure(0.0);
        assert!(db(early0 / 0.001) < -25.0, "no hold: {early0}");
    }

    #[test]
    fn slew_smears_onsets_and_converges_to_the_input_level() {
        let bin = f64::from(SR) / N as f64;
        let f = 50.0 * bin;
        let n = 48_000usize;
        let on = 4_000usize;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                if i < on {
                    0.0
                } else {
                    0.5 * (std::f64::consts::TAU * f * i as f64 / f64::from(SR)).sin() as f32
                }
            })
            .collect();
        let reach = |algo: SpectralAlgo| {
            let mut s = Spectral::new(SR, N);
            s.set_algorithm(algo);
            let (y, _) = run(&mut s, &x, &x);
            let win = 480;
            let mut t = None;
            let mut last = 0.0;
            for from in (0..n - win).step_by(48) {
                let a = amp(&y, f, from, from + win);
                last = a;
                if t.is_none() && a > 0.45 {
                    t = Some(from);
                }
            }
            (t.unwrap_or(n), last)
        };
        let (t_id, last_id) = reach(SpectralAlgo::Identity);
        let (t_slew, last_slew) = reach(SpectralAlgo::Slew { rise_db: 3.0 });
        assert!(t_slew > t_id + 3_000, "slew {t_slew} identity {t_id}");
        assert!(
            (last_slew - last_id).abs() < 0.01 && last_id > 0.49,
            "{last_slew} {last_id}"
        );
        // Falls pass at once: identical decay to the identity case.
        let (_, tail) = reach(SpectralAlgo::Slew { rise_db: 60.0 });
        assert!(tail > 0.49);
    }

    fn band_levels(algo: SpectralAlgo) -> (f64, f64) {
        let n = 16_384 * 6;
        let x = noise(7, n, 0.5);
        let mut s = Spectral::new(SR, N);
        s.set_algorithm(algo);
        let (y, _) = run(&mut s, &x, &x);
        let band_power = |sig: &[f32], lo: f64, hi: f64| {
            let mut acc = 0.0;
            for c in sig.chunks(16_384).filter(|c| c.len() == 16_384).skip(1) {
                let sp = spectrum(c);
                for (k, v) in sp.iter().enumerate() {
                    let f = k as f64 * f64::from(SR) / 16_384.0;
                    if f >= lo && f < hi {
                        acc += v * v;
                    }
                }
            }
            acc
        };
        let yy = &y[2 * N..];
        let xx = &x[..x.len() - 2 * N];
        (
            10.0 * (band_power(yy, 2_500.0, 3_500.0) / band_power(xx, 2_500.0, 3_500.0)).log10(),
            10.0 * (band_power(yy, 7_000.0, 9_000.0) / band_power(xx, 7_000.0, 9_000.0)).log10(),
        )
    }

    #[test]
    fn band_pass_keeps_and_band_stop_removes_the_requested_bins() {
        let (inside, outside) = band_levels(SpectralAlgo::BandPass {
            lo_hz: 2_000.0,
            hi_hz: 4_000.0,
        });
        assert!(inside.abs() < 1.0, "pass band {inside} dB");
        assert!(outside < -50.0, "stop region {outside} dB");
        let (inside, outside) = band_levels(SpectralAlgo::BandStop {
            lo_hz: 6_000.0,
            hi_hz: 10_000.0,
        });
        assert!(inside.abs() < 1.0, "kept band {inside} dB");
        assert!(outside < -50.0, "removed band {outside} dB");
    }

    #[test]
    fn quantise_rounds_levels_to_the_step() {
        let bin = f64::from(SR) / N as f64;
        let f = 70.0 * bin;
        let x = sine(f, f64::from(SR), 24_000, 0.3);
        let mut s = Spectral::new(SR, N);
        s.set_algorithm(SpectralAlgo::Quantise { step_db: 6.0 });
        let (y, _) = run(&mut s, &x, &x);
        let g = db(amp(&y, f, 12_000, 24_000) / 0.3);
        assert!(g.abs() <= 3.6, "within half a step: {g}");
        s.set_algorithm(SpectralAlgo::Quantise { step_db: 1.0 });
        s.reset();
        let (y, _) = run(&mut s, &x, &x);
        assert!(db(amp(&y, f, 12_000, 24_000) / 0.3).abs() <= 0.7);
        assert_eq!(s.algorithm(), SpectralAlgo::Quantise { step_db: 1.0 });
    }

    fn lossy(threshold_db: f32, hold_ms: f32, step_db: f32, dropout: f32) -> SpectralAlgo {
        SpectralAlgo::Lossy {
            threshold_db,
            hold_ms,
            step_db,
            dropout,
        }
    }

    #[test]
    fn lossy_with_everything_off_is_the_quantiser_alone() {
        // Gate at the bottom, no dropout: the same decision as the plain quantiser.
        let bin = f64::from(SR) / N as f64;
        let f = 70.0 * bin;
        let x = sine(f, f64::from(SR), 24_000, 0.3);
        let one = |algo: SpectralAlgo| {
            let mut s = Spectral::new(SR, N);
            s.set_algorithm(algo);
            run(&mut s, &x, &x).0
        };
        let (l, q) = (
            one(lossy(-120.0, 0.0, 6.0, 0.0)),
            one(SpectralAlgo::Quantise { step_db: 6.0 }),
        );
        let worst = l
            .iter()
            .zip(&q)
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn lossy_gates_quiet_bins_like_the_gate_and_quantises_the_rest() {
        let bin = f64::from(SR) / N as f64;
        let (fa, fb) = (40.0 * bin, 90.0 * bin);
        let n = 32_768;
        let a = sine(fa, f64::from(SR), n, 0.5);
        let b = sine(fb, f64::from(SR), n, 0.001);
        let x: Vec<f32> = a.iter().zip(&b).map(|(p, q)| p + q).collect();
        let mut s = Spectral::new(SR, N);
        s.set_algorithm(lossy(-40.0, 0.0, 1.0, 0.0));
        let (y, _) = run(&mut s, &x, &x);
        let (from, to) = (16_384, 32_768);
        // The loud tone passes within the quantiser's half step (0.5 dB).
        assert!(db(amp(&y, fa, from, to) / 0.5).abs() <= 0.6);
        // The quiet one is far below the threshold: attenuated hard.
        assert!(db(amp(&y, fb, from, to) / 0.001) < -30.0);
    }

    #[test]
    fn lossy_dropouts_remove_the_asked_share_of_the_energy_and_depend_on_the_seed_only() {
        let x = noise(9, 96_000, 0.3);
        let render = |seed: u64, p: f32| {
            let mut s = Spectral::new(SR, N);
            s.set_seed(seed);
            s.set_algorithm(lossy(-120.0, 0.0, 0.5, p));
            run(&mut s, &x, &x).0
        };
        let energy = |v: &[f32]| {
            v[4 * N..]
                .iter()
                .map(|a| f64::from(*a).powi(2))
                .sum::<f64>()
        };
        let reference = render(1, 0.0);
        let e0 = energy(&reference);
        let mut last = e0;
        for p in [0.25f32, 0.5, 0.75] {
            let y = render(1, p);
            // The coherent part (the projection on the undisturbed signal) is `1 - p` of it.
            let dot: f64 = y[4 * N..]
                .iter()
                .zip(&reference[4 * N..])
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            let kept = dot / e0;
            assert!(
                (kept - f64::from(1.0 - p)).abs() < 0.05,
                "dropout {p}: kept {kept}"
            );
            // The overall energy falls with the dropout probability.
            assert!(energy(&y) < last);
            last = energy(&y);
        }
        assert_eq!(render(3, 0.4), render(3, 0.4));
        assert_ne!(render(3, 0.4), render(4, 0.4));
        // Full dropout silences everything after the start.
        assert!(energy(&render(1, 1.0)) < 1e-12);
        // `reset` restores the start (the frame count restarts).
        let mut s = Spectral::new(SR, N);
        s.set_seed(7);
        s.set_algorithm(lossy(-120.0, 0.0, 0.5, 0.4));
        let first = run(&mut s, &x, &x).0;
        s.reset();
        assert_eq!(run(&mut s, &x, &x).0, first);
    }

    #[test]
    fn linked_decisions_keep_the_stereo_image() {
        let x = noise(5, 30_000, 0.05);
        let half: Vec<f32> = x.iter().map(|v| v * 0.5).collect();
        let mut s = Spectral::new(SR, N);
        s.set_algorithm(SpectralAlgo::Gate {
            threshold_db: -50.0,
            hold_ms: 5.0,
        });
        let (l, r) = run(&mut s, &x, &half);
        assert!(l.iter().zip(&r).all(|(a, b)| (b - a * 0.5).abs() <= 1e-7));
        // Unlinked, the two channels decide alone (the quieter one gates harder).
        let mut s = Spectral::new(SR, N);
        s.set_linked(false);
        s.set_algorithm(SpectralAlgo::Gate {
            threshold_db: -50.0,
            hold_ms: 0.0,
        });
        let (l, r) = run(&mut s, &x, &half);
        assert!(l.iter().zip(&r).any(|(a, b)| (b - a * 0.5).abs() > 1e-4));
    }

    #[test]
    fn non_finite_input_counts_as_silence_and_reset_restarts() {
        let mut s = Spectral::new(SR, 256);
        for _ in 0..2_000 {
            let (a, b) = s.process(f32::NAN, f32::INFINITY);
            assert_eq!((a, b), (0.0, 0.0));
        }
        let x = noise(1, 3_000, 0.5);
        let (a, _) = run(&mut Spectral::new(SR, 256), &x, &x);
        s.reset();
        let (b, _) = run(&mut s, &x, &x);
        assert_eq!(a, b, "a reset stage renders exactly like a fresh one");
    }

    #[test]
    fn process_is_chunk_invariant_for_every_algorithm() {
        let a = noise(4, 3_000, 0.3);
        let b = noise(5, 3_000, 0.3);
        for algo in [
            SpectralAlgo::Identity,
            SpectralAlgo::Gate {
                threshold_db: -30.0,
                hold_ms: 20.0,
            },
            SpectralAlgo::Slew { rise_db: 4.0 },
            SpectralAlgo::BandPass {
                lo_hz: 500.0,
                hi_hz: 5_000.0,
            },
            SpectralAlgo::BandStop {
                lo_hz: 500.0,
                hi_hz: 5_000.0,
            },
            SpectralAlgo::Quantise { step_db: 6.0 },
            lossy(-35.0, 15.0, 4.0, 0.2),
        ] {
            assert_chunk_invariant_stereo(
                || {
                    let mut s = Spectral::new(SR, 256);
                    s.set_algorithm(algo);
                    s
                },
                |s, x, y| s.process_block(x, y),
                &a,
                &b,
            );
        }
    }

    #[test]
    fn counter_wrap_does_not_change_the_output() {
        // Run far past the internal counter wrap and compare with a fresh stage on the tail.
        let x = noise(8, 10_000, 0.5);
        let mut s = Spectral::new(SR, 256);
        let (y, _) = run(&mut s, &x, &x);
        let lat = 256;
        assert!(err_db(&y[lat..], &x[..x.len() - lat]) < -90.0);
        assert!(rms(&y) > 0.1);
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_algorithm_and_parameters(
            which in 0usize..7, p1 in -80.0f32..10.0, p2 in 0.0f32..500.0,
            lo in 0.0f32..20_000.0, hi in 0.0f32..20_000.0, linked in proptest::bool::ANY,
            seed in 0u64..200
        ) {
            let algo = match which {
                0 => SpectralAlgo::Identity,
                1 => SpectralAlgo::Gate { threshold_db: p1, hold_ms: p2 },
                2 => SpectralAlgo::Slew { rise_db: p1.abs() },
                3 => SpectralAlgo::BandPass { lo_hz: lo, hi_hz: hi },
                4 => SpectralAlgo::BandStop { lo_hz: lo, hi_hz: hi },
                5 => SpectralAlgo::Quantise { step_db: p2 / 20.0 },
                _ => lossy(p1, p2, p2 / 20.0, lo / 20_000.0),
            };
            let mut s = Spectral::new(SR, 256);
            s.set_linked(linked);
            s.set_algorithm(algo);
            let x = noise(seed, 3_000, 1.0);
            let y = noise(seed + 1, 3_000, 1.0);
            let (l, r) = run(&mut s, &x, &y);
            proptest::prop_assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 20.0));
        }
    }
}
