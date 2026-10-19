//! Deterministic polyphase halfband oversampler: 1x (bypass), 2x and 4x, for non-linear
//! processing (distortion, true-peak detection, limiters). Mono; use one per channel.
//!
//! # Design
//!
//! Each factor-2 stage is a **linear-phase FIR halfband filter** (centre tap exactly 0.5, all
//! other even-offset taps exactly 0), designed once in [`Oversampler::new`] as a Kaiser-windowed
//! sinc. The design uses only IEEE `+ - * / sqrt` (the sine of the ideal halfband is exactly 0 or
//! +-1 at integer offsets, and the Kaiser `I0` is a power series), so the coefficients are
//! identical on every platform. Coefficients are `f32`; the sums run in `f32` in a fixed order.
//!
//! | Stage | Runs between | Taps (nonzero) | Passband / stopband (in base-rate Nyquist-relative terms) |
//! |---|---|---|---|
//! | 1 | base <-> 2x | 59 (31) | flat to 0.4 fs, stopband from 0.6 fs, Kaiser beta 9 |
//! | 2 | 2x <-> 4x | 23 padded to 25 (13) | flat to 0.4 fs, stopband from 1.6 fs (0.4 of the 4x rate), beta 9 |
//!
//! * **Up:** zero-stuff and filter, gain 2 (so passband gain is exactly 1), computed as two
//!   polyphase branches (only the nonzero taps).
//! * **Down:** filter and keep every second sample (only the nonzero taps), picking the even
//!   phase so the cascade delay is an integer number of base samples.
//! * **4x** cascades stage 1 then stage 2 going up, and stage 2 then stage 1 coming down.
//!
//! # Latency (base-rate samples, up + down)
//!
//! A stage with filter centre delay `c` (high-rate samples) delays a round trip by `c` samples of
//! its *low* rate. Stage 1 has `c = 29`; stage 2 is a 23-tap filter with 2 extra samples of delay
//! (`c = 12`, so the delay in base samples is `12 / 2 = 6`, an integer). Total:
//! [`LATENCY_2X`] = 29 and [`LATENCY_4X`] = 35 base samples; 1x has latency 0. The tests measure
//! the impulse delay.
//!
//! # Usage
//!
//! ```text
//! let hi = os.upsample(&input);      // len = input.len() * factor, internal buffer
//! for x in hi { *x = nonlinear(*x) }
//! os.downsample(&mut output);        // output.len() == input.len()
//! ```
//!
//! Nothing allocates after [`Oversampler::new`]. Changing the factor with
//! [`Oversampler::set_factor`] clears the filter state (the output of that batch is a short
//! transient, not a click-free crossfade).

/// Round-trip latency of the 2x chain in base-rate samples.
pub const LATENCY_2X: u32 = 29;
/// Round-trip latency of the 4x chain in base-rate samples.
pub const LATENCY_4X: u32 = 35;

/// Largest filter centre delay (taps = `2c + 1`).
const MAX_C: usize = 32;
/// Maximum nonzero taps of one stage.
const MAX_TAPS: usize = 40;

/// The oversampling factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OversampleFactor {
    /// No oversampling: the buffer is a copy and the latency is 0.
    X1,
    /// 2x.
    X2,
    /// 4x (two cascaded halfband stages).
    #[default]
    X4,
}

impl OversampleFactor {
    /// The integer ratio (1, 2 or 4).
    pub fn ratio(self) -> usize {
        match self {
            Self::X1 => 1,
            Self::X2 => 2,
            Self::X4 => 4,
        }
    }

    /// Round-trip latency in base-rate samples.
    pub fn latency_frames(self) -> u32 {
        match self {
            Self::X1 => 0,
            Self::X2 => LATENCY_2X,
            Self::X4 => LATENCY_4X,
        }
    }
}

/// Modified Bessel function `I0` by its power series (`+ * /` only).
fn bessel_i0(x: f64) -> f64 {
    let q = x * x * 0.25;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1.0;
    while k < 200.0 {
        term *= q / (k * k);
        sum += term;
        if term < sum * 1e-18 {
            break;
        }
        k += 1.0;
    }
    sum
}

/// Full impulse response `h[0..=2c]` of a halfband with odd taps up to offset `kmax`, centre at
/// `c` (`c >= kmax`; extra `c - kmax` is pure delay), Kaiser `beta`. Taps outside the nonzero set
/// are exactly 0.
fn halfband_taps(kmax: usize, c: usize, beta: f64) -> [f64; 2 * MAX_C + 1] {
    let mut h = [0.0f64; 2 * MAX_C + 1];
    let m = (kmax + 1) as f64;
    let i0b = bessel_i0(beta);
    let mut odd = [0.0f64; MAX_C + 1];
    let mut sum = 0.0;
    let mut j = 1;
    while j <= kmax {
        let r = j as f64 / m;
        let w = bessel_i0(beta * (1.0 - r * r).sqrt()) / i0b;
        let sign = if (j / 2) % 2 == 0 { 1.0 } else { -1.0 };
        odd[j] = sign / (std::f64::consts::PI * j as f64) * w;
        sum += odd[j];
        j += 2;
    }
    // Make the DC gain exactly 1: 0.5 + 2 * sum(odd) = 1.
    let scale = 0.25 / sum;
    h[c] = 0.5;
    let mut j = 1;
    while j <= kmax {
        h[c - j] = odd[j] * scale;
        h[c + j] = odd[j] * scale;
        j += 2;
    }
    h
}

/// Polyphase interpolator by 2 (input history, two output phases).
#[derive(Debug, Clone)]
struct HalfbandUp {
    hist: [f32; 32],
    pos: usize,
    n: [usize; 2],
    idx: [[u8; MAX_TAPS]; 2],
    coef: [[f32; MAX_TAPS]; 2],
}

impl HalfbandUp {
    fn new(h: &[f64; 2 * MAX_C + 1], c: usize) -> Self {
        let mut s = Self {
            hist: [0.0; 32],
            pos: 0,
            n: [0; 2],
            idx: [[0; MAX_TAPS]; 2],
            coef: [[0.0; MAX_TAPS]; 2],
        };
        for (k, &v) in h.iter().enumerate().take(2 * c + 1) {
            if v != 0.0 {
                let q = k % 2;
                let i = (k - q) / 2;
                let t = s.n[q];
                s.idx[q][t] = i as u8;
                s.coef[q][t] = (2.0 * v) as f32;
                s.n[q] += 1;
            }
        }
        s
    }

    fn reset(&mut self) {
        self.hist = [0.0; 32];
        self.pos = 0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> [f32; 2] {
        self.pos = (self.pos + 1) & 31;
        self.hist[self.pos] = x;
        let mut z = [0.0f32; 2];
        for (q, out) in z.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for t in 0..self.n[q] {
                let i = usize::from(self.idx[q][t]);
                acc += self.coef[q][t] * self.hist[(self.pos + 32 - i) & 31];
            }
            *out = acc;
        }
        z
    }
}

/// Polyphase decimator by 2.
#[derive(Debug, Clone)]
struct HalfbandDown {
    hist: [f32; 64],
    pos: usize,
    n: usize,
    idx: [u8; MAX_TAPS],
    coef: [f32; MAX_TAPS],
}

impl HalfbandDown {
    fn new(h: &[f64; 2 * MAX_C + 1], c: usize) -> Self {
        let mut s = Self {
            hist: [0.0; 64],
            pos: 0,
            n: 0,
            idx: [0; MAX_TAPS],
            coef: [0.0; MAX_TAPS],
        };
        for (k, &v) in h.iter().enumerate().take(2 * c + 1) {
            if v != 0.0 {
                s.idx[s.n] = k as u8;
                s.coef[s.n] = v as f32;
                s.n += 1;
            }
        }
        s
    }

    fn reset(&mut self) {
        self.hist = [0.0; 64];
        self.pos = 0;
    }

    /// Consumes two high-rate samples (`a` first, the even phase) and returns one output.
    #[inline]
    fn process(&mut self, a: f32, b: f32) -> f32 {
        self.pos = (self.pos + 1) & 63;
        self.hist[self.pos] = a;
        let mut acc = 0.0f32;
        for t in 0..self.n {
            acc += self.coef[t] * self.hist[(self.pos + 64 - usize::from(self.idx[t])) & 63];
        }
        self.pos = (self.pos + 1) & 63;
        self.hist[self.pos] = b;
        acc
    }
}

/// Mono 1x/2x/4x oversampler (see the module docs).
#[derive(Debug, Clone)]
pub struct Oversampler {
    factor: OversampleFactor,
    up1: HalfbandUp,
    up2: HalfbandUp,
    down1: HalfbandDown,
    down2: HalfbandDown,
    max_block: usize,
    buf: Vec<f32>,
}

impl Oversampler {
    /// Allocates for blocks of up to `max_block` base-rate samples and designs the filters
    /// (non-real-time). The default factor is 4x.
    pub fn new(max_block: usize) -> Self {
        let max_block = max_block.max(1);
        let h1 = halfband_taps(29, 29, 9.0);
        let h2 = halfband_taps(11, 12, 9.0);
        Self {
            factor: OversampleFactor::X4,
            up1: HalfbandUp::new(&h1, 29),
            up2: HalfbandUp::new(&h2, 12),
            down1: HalfbandDown::new(&h1, 29),
            down2: HalfbandDown::new(&h2, 12),
            max_block,
            buf: vec![0.0; max_block * 4],
        }
    }

    /// The current factor.
    pub fn factor(&self) -> OversampleFactor {
        self.factor
    }

    /// Selects the factor; clears the filter state when it changes. Allocation-free.
    pub fn set_factor(&mut self, factor: OversampleFactor) {
        if factor != self.factor {
            self.factor = factor;
            self.reset();
        }
    }

    /// Clears all filter state.
    pub fn reset(&mut self) {
        self.up1.reset();
        self.up2.reset();
        self.down1.reset();
        self.down2.reset();
        self.buf.fill(0.0);
    }

    /// Round-trip latency of the current factor in base-rate samples.
    pub fn latency_frames(&self) -> u32 {
        self.factor.latency_frames()
    }

    /// Largest block (base-rate samples) accepted.
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Upsamples `input` (at most [`Oversampler::max_block`] samples; extra samples are
    /// ignored) and returns the internal buffer holding `len * factor` samples for in-place
    /// processing. Call [`Oversampler::downsample`] with an output of the same length afterwards.
    pub fn upsample(&mut self, input: &[f32]) -> &mut [f32] {
        let n = input.len().min(self.max_block);
        match self.factor {
            OversampleFactor::X1 => {
                self.buf[..n].copy_from_slice(&input[..n]);
                &mut self.buf[..n]
            }
            OversampleFactor::X2 => {
                for (i, &x) in input[..n].iter().enumerate() {
                    let z = self.up1.process(x);
                    self.buf[2 * i] = z[0];
                    self.buf[2 * i + 1] = z[1];
                }
                &mut self.buf[..2 * n]
            }
            OversampleFactor::X4 => {
                for (i, &x) in input[..n].iter().enumerate() {
                    let z = self.up1.process(x);
                    let a = self.up2.process(z[0]);
                    let b = self.up2.process(z[1]);
                    self.buf[4 * i..4 * i + 4].copy_from_slice(&[a[0], a[1], b[0], b[1]]);
                }
                &mut self.buf[..4 * n]
            }
        }
    }

    /// Filters and decimates the internal buffer (as left by the caller after
    /// [`Oversampler::upsample`]) into `output`; `output.len()` must equal the input length of
    /// the matching `upsample` call.
    pub fn downsample(&mut self, output: &mut [f32]) {
        let n = output.len().min(self.max_block);
        match self.factor {
            OversampleFactor::X1 => output[..n].copy_from_slice(&self.buf[..n]),
            OversampleFactor::X2 => {
                for (i, y) in output[..n].iter_mut().enumerate() {
                    *y = self.down1.process(self.buf[2 * i], self.buf[2 * i + 1]);
                }
            }
            OversampleFactor::X4 => {
                for (i, y) in output[..n].iter_mut().enumerate() {
                    let b = &self.buf[4 * i..4 * i + 4];
                    let a0 = self.down2.process(b[0], b[1]);
                    let a1 = self.down2.process(b[2], b[3]);
                    *y = self.down1.process(a0, a1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::{FftPlanner, num_complex::Complex};

    const N: usize = 8192;
    const BLOCK: usize = 64;

    fn run(os: &mut Oversampler, x: &[f32]) -> Vec<f32> {
        let mut y = vec![0.0f32; x.len()];
        for (xi, yi) in x.chunks(BLOCK).zip(y.chunks_mut(BLOCK)) {
            os.upsample(xi);
            os.downsample(yi);
        }
        y
    }

    fn sine(k: usize, n: usize, amp: f64) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (std::f64::consts::TAU * (k * i % n) as f64 / n as f64).sin()) as f32)
            .collect()
    }

    #[test]
    fn taps_are_halfband_and_symmetric() {
        let h = halfband_taps(29, 29, 9.0);
        assert_eq!(h[29].to_bits(), 0.5f64.to_bits());
        for j in 1..=29usize {
            assert_eq!(h[29 - j].to_bits(), h[29 + j].to_bits());
            if j % 2 == 0 {
                assert_eq!(h[29 + j], 0.0);
            } else {
                assert!(h[29 + j] != 0.0);
            }
        }
        let sum: f64 = h.iter().sum();
        assert!((sum - 1.0).abs() < 1e-12, "{sum}");
        // The padded stage-2 filter has its taps at 1..=23 around centre 12.
        let h2 = halfband_taps(11, 12, 9.0);
        assert_eq!(h2[12].to_bits(), 0.5f64.to_bits());
        assert_eq!(h2[0].to_bits(), 0);
        assert!(h2[1].to_bits() != 0);
        assert!(h2[23].to_bits() != 0);
        assert_eq!(h2[24].to_bits(), 0);
    }

    #[test]
    fn latency_is_exact_for_every_factor() {
        for (factor, want) in [
            (OversampleFactor::X1, 0usize),
            (OversampleFactor::X2, LATENCY_2X as usize),
            (OversampleFactor::X4, LATENCY_4X as usize),
        ] {
            let mut os = Oversampler::new(BLOCK);
            os.set_factor(factor);
            assert_eq!(os.latency_frames() as usize, want);
            let mut x = vec![0.0f32; 512];
            x[7] = 1.0;
            let y = run(&mut os, &x);
            let (peak_at, peak) = y.iter().enumerate().fold((0, 0.0f32), |m, (i, v)| {
                if v.abs() > m.1 { (i, v.abs()) } else { m }
            });
            println!(
                "{factor:?}: impulse peak at +{} value {peak:.6}",
                peak_at - 7
            );
            assert_eq!(peak_at, 7 + want, "{factor:?}");
            // A full-band impulse loses its out-of-band part: the peak is slightly below 1.
            assert!(peak > 0.9 && peak < 1.01, "{factor:?} peak {peak}");
            // Linear phase: the response is symmetric about the peak.
            if want > 0 {
                for d in 1..want.min(20) {
                    let (a, b) = (y[peak_at - d], y[peak_at + d]);
                    assert!((a - b).abs() < 1e-5, "{factor:?} d={d}: {a} vs {b}");
                }
            }
        }
    }

    #[test]
    fn x1_is_a_bit_exact_copy() {
        let mut os = Oversampler::new(BLOCK);
        os.set_factor(OversampleFactor::X1);
        let x = sine(37, 256, 0.7);
        let y = run(&mut os, &x);
        assert!(x.iter().zip(&y).all(|(a, b)| a.to_bits() == b.to_bits()));
    }

    #[test]
    fn passband_is_flat_to_point_four_fs() {
        for factor in [OversampleFactor::X2, OversampleFactor::X4] {
            let mut worst = 0.0f64;
            for k in [100, 400, 800, 1200, 1600, 3000, 3270] {
                // k / N cycles per sample: up to 0.399 fs.
                let mut os = Oversampler::new(BLOCK);
                os.set_factor(factor);
                let x = sine(k, N, 1.0);
                let y = run(&mut os, &x);
                // Measure the second half (settled), whole cycles of the same bin count.
                // The window (second half) spans k / 2 whole cycles (k is even).
                let a = bin_amp_window(&y[N / 2..], k, N, N / 2);
                let db = 20.0 * a.log10();
                worst = worst.max(db.abs());
                assert!(db.abs() < 0.1, "{factor:?} k={k}: {db:+.4} dB");
            }
            println!("{factor:?}: worst passband deviation {worst:.5} dB (to 0.399 fs)");
        }
    }

    /// Amplitude at `k` cycles per `period` samples over `y`, where `y[0]` is absolute sample
    /// `start` (exact when the window spans whole cycles).
    fn bin_amp_window(y: &[f32], k: usize, period: usize, start: usize) -> f64 {
        let (mut s, mut c) = (0.0f64, 0.0f64);
        for (i, v) in y.iter().enumerate() {
            let ph = std::f64::consts::TAU * (k * (start + i) % period) as f64 / period as f64;
            s += f64::from(*v) * ph.sin();
            c += f64::from(*v) * ph.cos();
        }
        2.0 * (s * s + c * c).sqrt() / y.len() as f64
    }

    /// Worst non-fundamental level (dB re the fundamental) of the Hann-windowed spectrum.
    fn worst_spur_db(y: &[f32], fund_bin: usize, guard: usize) -> f64 {
        let n = y.len();
        let mut buf: Vec<Complex<f64>> = y
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
                Complex::new(f64::from(*s) * w, 0.0)
            })
            .collect();
        FftPlanner::new().plan_fft_forward(n).process(&mut buf);
        let mags: Vec<f64> = buf[..n / 2].iter().map(|c| c.norm()).collect();
        let fund = mags[fund_bin];
        let spur = mags
            .iter()
            .enumerate()
            .filter(|(k, _)| k.abs_diff(fund_bin) > guard)
            .fold(0.0f64, |m, (_, v)| m.max(*v));
        20.0 * (spur / fund).log10()
    }

    #[test]
    fn upsampler_images_are_rejected() {
        // A tone in the passband (0.35 fs and 0.40 fs): every image in the 4x output (31 kHz,
        // 65 kHz, ... at 48 kHz base) must be at least 80 dB down.
        for k in [2867usize, 3277] {
            for factor in [OversampleFactor::X2, OversampleFactor::X4] {
                let mut os = Oversampler::new(N);
                os.set_factor(factor);
                // Prime the filters, then capture one block.
                let x = sine(k, N, 1.0);
                os.upsample(&x);
                let hi: Vec<f32> = os.upsample(&x).to_vec();
                let spur = worst_spur_db(&hi, k, 6);
                println!("{factor:?} tone {k}/{N}: worst image {spur:.1} dB");
                assert!(spur < -80.0, "{factor:?} k={k}: {spur:.1} dB");
            }
        }
    }

    /// Gain (dB) of the down chain for a tone at `k` cycles per `n_hi` high-rate samples.
    fn down_gain_db(factor: OversampleFactor, k: usize) -> f64 {
        let r = factor.ratio();
        let n_base = 4096usize;
        let n_hi = n_base * r;
        let mut os = Oversampler::new(n_base);
        os.set_factor(factor);
        let zeros = vec![0.0f32; n_base];
        // Settle a first block, then measure a second one.
        for pass in 0..2 {
            let hi = os.upsample(&zeros);
            for (i, v) in hi.iter_mut().enumerate() {
                let ph =
                    std::f64::consts::TAU * (k * ((pass * n_hi + i) % n_hi)) as f64 / n_hi as f64;
                *v = ph.sin() as f32;
            }
            let mut out = vec![0.0f32; n_base];
            os.downsample(&mut out);
            if pass == 1 {
                // The folded tone lands on bin (k mod n_base) folded into 0..n_base/2.
                let rms =
                    (out.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / n_base as f64).sqrt();
                return 20.0 * (rms * std::f64::consts::SQRT_2).log10();
            }
        }
        unreachable!()
    }

    #[test]
    fn downsampler_alias_rejection() {
        // Tones that fold into the passband must be at least 80 dB down.
        // 2x: 0.8 fs (= 0.4 of the 2x rate) folds to 0.2 fs; 4x: 1.8 fs folds to 0.2 fs.
        let mut worst = f64::NEG_INFINITY;
        for step in 0..30 {
            // Stopband of the 2x chain: 0.6 .. 1.0 fs at the 2x rate: bins of 8192-sample block.
            let f = 0.6 + 0.4 * f64::from(step) / 29.0; // cycles per base sample
            let k = (f * 4096.0).round() as usize; // cycles per block of 4096 base samples
            let g = down_gain_db(OversampleFactor::X2, k);
            worst = worst.max(g);
        }
        println!("2x down chain: worst stopband gain {worst:.1} dB");
        assert!(worst < -80.0, "2x worst {worst:.1} dB");
        let mut worst4 = f64::NEG_INFINITY;
        // 4x: frequencies (cycles per base sample) in [0.6, 1.4] and [1.6, 2.0).
        for step in 0..60 {
            let f = if step < 40 {
                0.6 + 0.8 * f64::from(step) / 39.0
            } else {
                1.6 + 0.399 * f64::from(step - 40) / 19.0
            };
            let k = (f * 4096.0).round() as usize;
            let g = down_gain_db(OversampleFactor::X4, k);
            worst4 = worst4.max(g);
        }
        println!("4x down chain: worst stopband gain {worst4:.1} dB");
        assert!(worst4 < -80.0, "4x worst {worst4:.1} dB");
    }

    #[test]
    fn round_trip_of_a_band_limited_signal() {
        // Two tones at 0.1 and 0.3 fs: output equals the input delayed by the latency.
        for factor in [OversampleFactor::X2, OversampleFactor::X4] {
            let mut os = Oversampler::new(BLOCK);
            os.set_factor(factor);
            let a = sine(410, N, 0.4);
            let b = sine(1229, N, 0.3);
            let x: Vec<f32> = a.iter().zip(&b).map(|(p, q)| p + q).collect();
            let y = run(&mut os, &x);
            let lat = factor.latency_frames() as usize;
            let mut worst = 0.0f32;
            for i in 2 * N / 4..N {
                worst = worst.max((y[i] - x[i - lat]).abs());
            }
            println!("{factor:?}: round-trip max error {worst:.2e}");
            assert!(worst < 2e-3, "{factor:?}: {worst}");
        }
    }

    #[test]
    fn is_deterministic_and_resets() {
        let x = sine(777, 1024, 0.9);
        let mut a = Oversampler::new(BLOCK);
        let mut b = Oversampler::new(BLOCK);
        let ya = run(&mut a, &x);
        let yb = run(&mut b, &x);
        assert!(ya.iter().zip(&yb).all(|(p, q)| p.to_bits() == q.to_bits()));
        a.reset();
        let yc = run(&mut a, &x);
        assert!(ya.iter().zip(&yc).all(|(p, q)| p.to_bits() == q.to_bits()));
        // A factor change resets too.
        a.set_factor(OversampleFactor::X2);
        a.set_factor(OversampleFactor::X4);
        let yd = run(&mut a, &x);
        assert!(ya.iter().zip(&yd).all(|(p, q)| p.to_bits() == q.to_bits()));
    }
}
