//! Deterministic polyphase halfband oversampler: 1x (bypass), 2x, 4x and 8x, for non-linear
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
//! | 3 | 4x <-> 8x | 15 padded to 17 (8) | flat to 0.4 fs, stopband from 3.6 fs, beta 9 |
//!
//! * **Up:** zero-stuff and filter, gain 2 (so passband gain is exactly 1), computed as two
//!   polyphase branches (only the nonzero taps).
//! * **Down:** filter and keep every second sample (only the nonzero taps), picking the even
//!   phase so the cascade delay is an integer number of base samples.
//! * **4x** cascades stage 1 then stage 2 going up, and stage 2 then stage 1 coming down.
//! * **8x** adds stage 3 after stage 2 going up and before it coming down.
//!
//! # How the sums run
//!
//! A block goes through one stage at a time, in passes of at most 64 base-rate samples, over a
//! linear history (the last 32 samples of the stage, then the block). The taps are the outer loop
//! and the samples of the pass the inner one, so a tap is one multiply-add over contiguous
//! slices, which the compiler turns into vector code. Each output still receives the additions
//! of the same products in the same tap order, starting from `+0.0`, as the sample-by-sample
//! form did (the tests compare against that form bit for bit), so the result does not depend
//! on how the caller cuts the stream into blocks.
//!
//! # Latency (base-rate samples, up + down)
//!
//! A stage with filter centre delay `c` (high-rate samples) delays a round trip by `c` samples of
//! its *low* rate. Stage 1 has `c = 29`; stage 2 is a 23-tap filter with 2 extra samples of delay
//! (`c = 12`, so the delay in base samples is `12 / 2 = 6`, an integer); stage 3 has `c = 8` at
//! the 4x rate, which is 2 base samples. Total: [`LATENCY_2X`] = 29, [`LATENCY_4X`] = 35 and
//! [`LATENCY_8X`] = 37 base samples; 1x has latency 0. The tests measure
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
/// Round-trip latency of the 8x chain in base-rate samples.
pub const LATENCY_8X: u32 = 37;

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
    /// 8x (three cascaded halfband stages).
    X8,
}

impl OversampleFactor {
    /// The integer ratio (1, 2, 4 or 8).
    pub fn ratio(self) -> usize {
        match self {
            Self::X1 => 1,
            Self::X2 => 2,
            Self::X4 => 4,
            Self::X8 => 8,
        }
    }

    /// Round-trip latency in base-rate samples.
    pub fn latency_frames(self) -> u32 {
        match self {
            Self::X1 => 0,
            Self::X2 => LATENCY_2X,
            Self::X4 => LATENCY_4X,
            Self::X8 => LATENCY_8X,
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

/// Samples of history kept in front of every stage's block: at least the largest tap delay (29).
const HIST: usize = 32;
/// Base-rate samples a pass handles at most. A longer block is cut into passes of this size, so
/// the working buffers do not grow with `max_block` and stay in the first-level cache.
const CHUNK: usize = 64;

// How the block form keeps the bits of the per-sample form it replaced.
//
// Every output sample is `acc = 0.0; acc += coef[t] * x[..]` for the taps `t` in the order of
// `idx` / `coef`. Here the taps are the outer loop and the samples of the block the inner one:
// the running sum of every output starts at `+0.0` and each tap, the first included, adds its
// `coef * x` to it. An output therefore receives exactly the same additions of the same products
// in the same tap order, and the outputs do not depend on each other. Splitting a block anywhere
// (the passes of `CHUNK`, the calls of the caller) cannot change a bit, because the history in
// front of a block is the same samples the ring held. Nothing is fused or reassociated, and no
// sum is taken in another order.

/// `out[j] += c * src[j]` for every `j`: one tap over a block (equal lengths, so no bounds check
/// is left in the loop and it vectorises; each lane is the scalar `+` of a scalar `*`).
#[inline(always)]
fn add_scaled(out: &mut [f32], c: f32, src: &[f32]) {
    debug_assert_eq!(out.len(), src.len());
    for (o, &s) in out.iter_mut().zip(src) {
        *o += c * s;
    }
}

/// Outputs a tile of `fir` keeps in registers while it runs through the taps.
const TILE: usize = 16;

/// `out[j] = 0.0 + c0 * src0[j] + c1 * src1[j] + ...` over the taps `(c, src)` in the order the
/// iterator gives them, every term added in turn to a running sum that starts at `+0.0`.
#[inline(always)]
fn fir<'a, I>(out: &mut [f32], taps: I)
where
    I: Iterator<Item = (f32, &'a [f32])> + Clone,
{
    let main = out.len() - out.len() % TILE;
    for (j, tile) in out[..main].as_chunks_mut::<TILE>().0.iter_mut().enumerate() {
        let mut acc = [0.0f32; TILE];
        for (c, src) in taps.clone() {
            for (a, &v) in acc.iter_mut().zip(&src[j * TILE..(j + 1) * TILE]) {
                *a += c * v;
            }
        }
        *tile = acc;
    }
    let tail = &mut out[main..];
    tail.fill(0.0);
    for (c, src) in taps {
        add_scaled(tail, c, &src[main..]);
    }
}

/// Polyphase interpolator by 2: the input history is the first `HIST` samples of `lin`, the
/// block follows it.
#[derive(Debug, Clone)]
struct HalfbandUp {
    n: [usize; 2],
    idx: [[u8; MAX_TAPS]; 2],
    coef: [[f32; MAX_TAPS]; 2],
    lin: Vec<f32>,
}

impl HalfbandUp {
    /// A stage for blocks of up to `cap` input samples.
    fn new(h: &[f64; 2 * MAX_C + 1], c: usize, cap: usize) -> Self {
        let mut s = Self {
            n: [0; 2],
            idx: [[0; MAX_TAPS]; 2],
            coef: [[0.0; MAX_TAPS]; 2],
            lin: vec![0.0; HIST + cap],
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
        self.lin[..HIST].fill(0.0);
    }

    /// The `m` input samples of the pass are in `lin[HIST..HIST + m]`. Writes the two output
    /// phases interleaved to `dst[..2 * m]`, using `scratch[..2 * m]`, and keeps the last `HIST`
    /// samples for the next pass.
    fn run(&mut self, m: usize, scratch: &mut [f32], dst: &mut [f32]) {
        let (p0, rest) = scratch.split_at_mut(m);
        let phases = [p0, &mut rest[..m]];
        for (q, out) in phases.into_iter().enumerate() {
            let lin = &self.lin;
            let taps = self.idx[q].iter().zip(&self.coef[q]).take(self.n[q]);
            fir(
                out,
                taps.map(|(&i, &c)| (c, &lin[HIST - usize::from(i)..][..m])),
            );
        }
        let (p0, p1) = (&scratch[..m], &scratch[m..2 * m]);
        for ((d, &a), &b) in dst[..2 * m]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(p0)
            .zip(p1)
        {
            d[0] = a;
            d[1] = b;
        }
        self.lin.copy_within(m..m + HIST, 0);
    }
}

/// Polyphase decimator by 2: the two input phases (the even samples `a`, the odd samples `b`)
/// each keep `HIST` samples of history in front of the block.
#[derive(Debug, Clone)]
struct HalfbandDown {
    n: usize,
    idx: [u8; MAX_TAPS],
    coef: [f32; MAX_TAPS],
    lin_a: Vec<f32>,
    lin_b: Vec<f32>,
}

impl HalfbandDown {
    /// A stage for blocks of up to `cap` output samples.
    fn new(h: &[f64; 2 * MAX_C + 1], c: usize, cap: usize) -> Self {
        let mut s = Self {
            n: 0,
            idx: [0; MAX_TAPS],
            coef: [0.0; MAX_TAPS],
            lin_a: vec![0.0; HIST + cap],
            lin_b: vec![0.0; HIST + cap],
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
        self.lin_a[..HIST].fill(0.0);
        self.lin_b[..HIST].fill(0.0);
    }

    /// Filters `src` (`2 * out.len()` high-rate samples, the even one of each pair first) into
    /// `out`. Output `j` has the even sample `a_j` as its newest: tap `k` reads the sample `k`
    /// places back, which is `a[j - k / 2]` for an even `k` and `b[j - (k + 1) / 2]` for an odd
    /// one (the centre tap of the padded stages is the only one that changes phase, and the
    /// taps are visited in their own order).
    fn run(&mut self, src: &[f32], out: &mut [f32]) {
        let m = out.len();
        debug_assert_eq!(src.len(), 2 * m);
        let (a, b) = (
            &mut self.lin_a[HIST..HIST + m],
            &mut self.lin_b[HIST..HIST + m],
        );
        for ((x, y), pair) in a.iter_mut().zip(b.iter_mut()).zip(src.as_chunks::<2>().0) {
            *x = pair[0];
            *y = pair[1];
        }
        let taps = self.idx.iter().zip(&self.coef).take(self.n);
        fir(
            out,
            taps.map(|(&k, &c)| {
                let k = usize::from(k);
                let lin = if k & 1 == 0 { &self.lin_a } else { &self.lin_b };
                (c, &lin[HIST - ((k + 1) >> 1)..][..m])
            }),
        );
        self.lin_a.copy_within(m..m + HIST, 0);
        self.lin_b.copy_within(m..m + HIST, 0);
    }
}

/// Mono 1x/2x/4x/8x oversampler (see the module docs).
#[derive(Debug, Clone)]
pub struct Oversampler {
    factor: OversampleFactor,
    up1: HalfbandUp,
    up2: HalfbandUp,
    up3: HalfbandUp,
    down1: HalfbandDown,
    down2: HalfbandDown,
    down3: HalfbandDown,
    max_block: usize,
    buf: Vec<f32>,
    /// Working space of a pass: the phases of an up stage, the output of a down stage.
    scratch: Vec<f32>,
}

impl Oversampler {
    /// Allocates for blocks of up to `max_block` base-rate samples and designs the filters
    /// (non-real-time). The default factor is 4x.
    pub fn new(max_block: usize) -> Self {
        let max_block = max_block.max(1);
        let cap = max_block.min(CHUNK);
        let h1 = halfband_taps(29, 29, 9.0);
        let h2 = halfband_taps(11, 12, 9.0);
        let h3 = halfband_taps(7, 8, 9.0);
        Self {
            factor: OversampleFactor::X4,
            up1: HalfbandUp::new(&h1, 29, cap),
            up2: HalfbandUp::new(&h2, 12, 2 * cap),
            up3: HalfbandUp::new(&h3, 8, 4 * cap),
            down1: HalfbandDown::new(&h1, 29, cap),
            down2: HalfbandDown::new(&h2, 12, 2 * cap),
            down3: HalfbandDown::new(&h3, 8, 4 * cap),
            max_block,
            buf: vec![0.0; max_block * 8],
            scratch: vec![0.0; cap * 8],
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
        self.up3.reset();
        self.down1.reset();
        self.down2.reset();
        self.down3.reset();
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
        let ratio = self.factor.ratio();
        let Self {
            factor,
            up1,
            up2,
            up3,
            buf,
            scratch,
            ..
        } = self;
        if *factor == OversampleFactor::X1 {
            buf[..n].copy_from_slice(&input[..n]);
            return &mut buf[..n];
        }
        for (x, dst) in input[..n]
            .chunks(CHUNK)
            .zip(buf[..n * ratio].chunks_mut(CHUNK * ratio))
        {
            let m = x.len();
            up1.lin[HIST..HIST + m].copy_from_slice(x);
            match *factor {
                OversampleFactor::X1 | OversampleFactor::X2 => up1.run(m, scratch, dst),
                OversampleFactor::X4 => {
                    up1.run(m, scratch, &mut up2.lin[HIST..HIST + 2 * m]);
                    up2.run(2 * m, scratch, dst);
                }
                OversampleFactor::X8 => {
                    up1.run(m, scratch, &mut up2.lin[HIST..HIST + 2 * m]);
                    up2.run(2 * m, scratch, &mut up3.lin[HIST..HIST + 4 * m]);
                    up3.run(4 * m, scratch, dst);
                }
            }
        }
        &mut buf[..n * ratio]
    }

    /// Filters and decimates the internal buffer (as left by the caller after
    /// [`Oversampler::upsample`]) into `output`; `output.len()` must equal the input length of
    /// the matching `upsample` call.
    pub fn downsample(&mut self, output: &mut [f32]) {
        let n = output.len().min(self.max_block);
        let ratio = self.factor.ratio();
        let Self {
            factor,
            down1,
            down2,
            down3,
            buf,
            scratch,
            ..
        } = self;
        if *factor == OversampleFactor::X1 {
            output[..n].copy_from_slice(&buf[..n]);
            return;
        }
        let half = scratch.len() / 2;
        let (s4, s2) = scratch.split_at_mut(half);
        for (y, src) in output[..n]
            .chunks_mut(CHUNK)
            .zip(buf[..n * ratio].chunks(CHUNK * ratio))
        {
            let m = y.len();
            match *factor {
                OversampleFactor::X1 | OversampleFactor::X2 => down1.run(src, y),
                OversampleFactor::X4 => {
                    down2.run(src, &mut s2[..2 * m]);
                    down1.run(&s2[..2 * m], y);
                }
                OversampleFactor::X8 => {
                    down3.run(src, &mut s4[..4 * m]);
                    down2.run(&s4[..4 * m], &mut s2[..2 * m]);
                    down1.run(&s2[..2 * m], y);
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
            (OversampleFactor::X8, LATENCY_8X as usize),
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
        for factor in [
            OversampleFactor::X2,
            OversampleFactor::X4,
            OversampleFactor::X8,
        ] {
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
            for factor in [
                OversampleFactor::X2,
                OversampleFactor::X4,
                OversampleFactor::X8,
            ] {
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
        for factor in [
            OversampleFactor::X2,
            OversampleFactor::X4,
            OversampleFactor::X8,
        ] {
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

    // ---- The per-sample form the block form replaced, kept as the reference ----

    /// The oversampler as it was before the block form: every sample through a ring buffer, the
    /// stages interleaved sample by sample. Its arithmetic is unchanged; the block form has to
    /// give the same bits for every input, factor and block length.
    mod reference {
        use super::{MAX_C, MAX_TAPS, OversampleFactor, halfband_taps};

        #[derive(Clone)]
        struct Up {
            hist: [f32; 32],
            pos: usize,
            n: [usize; 2],
            idx: [[u8; MAX_TAPS]; 2],
            coef: [[f32; MAX_TAPS]; 2],
        }

        impl Up {
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

        #[derive(Clone)]
        struct Down {
            hist: [f32; 64],
            pos: usize,
            n: usize,
            idx: [u8; MAX_TAPS],
            coef: [f32; MAX_TAPS],
        }

        impl Down {
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

            fn process(&mut self, a: f32, b: f32) -> f32 {
                self.pos = (self.pos + 1) & 63;
                self.hist[self.pos] = a;
                let mut acc = 0.0f32;
                for t in 0..self.n {
                    acc +=
                        self.coef[t] * self.hist[(self.pos + 64 - usize::from(self.idx[t])) & 63];
                }
                self.pos = (self.pos + 1) & 63;
                self.hist[self.pos] = b;
                acc
            }
        }

        #[derive(Clone)]
        pub struct Oversampler {
            factor: OversampleFactor,
            up1: Up,
            up2: Up,
            up3: Up,
            down1: Down,
            down2: Down,
            down3: Down,
            max_block: usize,
            buf: Vec<f32>,
        }

        impl Oversampler {
            pub fn new(max_block: usize) -> Self {
                let max_block = max_block.max(1);
                let h1 = halfband_taps(29, 29, 9.0);
                let h2 = halfband_taps(11, 12, 9.0);
                let h3 = halfband_taps(7, 8, 9.0);
                Self {
                    factor: OversampleFactor::X4,
                    up1: Up::new(&h1, 29),
                    up2: Up::new(&h2, 12),
                    up3: Up::new(&h3, 8),
                    down1: Down::new(&h1, 29),
                    down2: Down::new(&h2, 12),
                    down3: Down::new(&h3, 8),
                    max_block,
                    buf: vec![0.0; max_block * 8],
                }
            }

            pub fn set_factor(&mut self, factor: OversampleFactor) {
                if factor != self.factor {
                    self.factor = factor;
                    self.reset();
                }
            }

            pub fn reset(&mut self) {
                self.up1.reset();
                self.up2.reset();
                self.up3.reset();
                self.down1.reset();
                self.down2.reset();
                self.down3.reset();
                self.buf.fill(0.0);
            }

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
                    OversampleFactor::X8 => {
                        for (i, &x) in input[..n].iter().enumerate() {
                            let z = self.up1.process(x);
                            let a = self.up2.process(z[0]);
                            let b = self.up2.process(z[1]);
                            for (j, q) in [a[0], a[1], b[0], b[1]].into_iter().enumerate() {
                                let w = self.up3.process(q);
                                self.buf[8 * i + 2 * j] = w[0];
                                self.buf[8 * i + 2 * j + 1] = w[1];
                            }
                        }
                        &mut self.buf[..8 * n]
                    }
                }
            }

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
                    OversampleFactor::X8 => {
                        for (i, y) in output[..n].iter_mut().enumerate() {
                            let b = &self.buf[8 * i..8 * i + 8];
                            let m0 = self.down3.process(b[0], b[1]);
                            let m1 = self.down3.process(b[2], b[3]);
                            let m2 = self.down3.process(b[4], b[5]);
                            let m3 = self.down3.process(b[6], b[7]);
                            let a0 = self.down2.process(m0, m1);
                            let a1 = self.down2.process(m2, m3);
                            *y = self.down1.process(a0, a1);
                        }
                    }
                }
            }
        }
    }

    // ---- Bit-for-bit comparison against the reference ----

    /// A tiny deterministic generator (no dependency, the same on every platform).
    struct Rng(u32);

    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            self.0 >> 8
        }

        /// In `-1.0..1.0`.
        fn unit(&mut self) -> f32 {
            self.next() as f32 / (1u32 << 23) as f32 - 1.0
        }

        fn below(&mut self, n: usize) -> usize {
            self.next() as usize % n
        }
    }

    /// Samples per segment of the test stream; the stream holds `SEGMENTS` of them.
    const SEG: usize = 700;
    const SEGMENTS: usize = 8;

    /// A deterministic stream of `SEGMENTS * SEG * scale` samples: noise; zeros between bursts,
    /// with `-0.0` and exact zeros among them; impulses and short `-0.0` runs; values near
    /// `f32::MAX / 64`; subnormals; a mix of everything; long runs of `-0.0`; noise again.
    fn stream(seed: u32, scale: usize) -> Vec<f32> {
        let mut r = Rng(seed);
        let mut out = Vec::with_capacity(SEGMENTS * SEG * scale);
        for kind in 0..SEGMENTS {
            let mut burst_left = 0usize;
            for i in 0..SEG * scale {
                let v = match kind {
                    0 => 0.9 * r.unit(),
                    1 => {
                        // Runs of zeros (both signs) between bursts of noise.
                        if burst_left == 0 && r.below(150) == 0 {
                            burst_left = 5 + r.below(60);
                        }
                        if burst_left > 0 {
                            burst_left -= 1;
                            0.5 * r.unit()
                        } else if r.below(3) == 0 {
                            -0.0
                        } else {
                            0.0
                        }
                    }
                    2 => match i % 97 {
                        0 => 1.0,
                        1 => -1.0,
                        50..=60 => -0.0,
                        _ => 0.0,
                    },
                    3 => {
                        if r.below(4) == 0 {
                            0.0
                        } else {
                            r.unit() * (f32::MAX / 64.0)
                        }
                    }
                    4 => match r.below(5) {
                        0 => 0.0,
                        1 => f32::from_bits(1 + r.next() % 0x7f_ffff),
                        2 => -f32::from_bits(1 + r.next() % 0x7f_ffff),
                        3 => r.unit() * 1.0e-30,
                        _ => f32::MIN_POSITIVE * r.unit(),
                    },
                    5 => match r.below(6) {
                        0 => 0.0,
                        1 => -0.0,
                        2 => r.unit() * (f32::MAX / 64.0),
                        3 => f32::from_bits(r.next() % 0x7f_ffff),
                        _ => r.unit(),
                    },
                    6 => {
                        // Long runs of `-0.0` (every product is `-0.0`) between single samples.
                        if i % 300 == 299 { 0.25 } else { -0.0 }
                    }
                    _ => 0.3 * r.unit(),
                };
                out.push(v);
            }
        }
        out
    }

    /// Block lengths for a stream of at least `total` samples at a given `max_block`: 0, 1, 2, 3,
    /// every length up to 64 (and up to `max_block`), then a seeded run of random lengths that
    /// reach past `max_block` now and then (the extra is ignored).
    fn block_lengths(max_block: usize, total: usize, seed: u32) -> Vec<usize> {
        let mut r = Rng(seed);
        let mut lens = vec![0, 1, 2, 3, 0, 0, 1];
        lens.extend(0..=64.min(max_block));
        let mut sum: usize = lens.iter().map(|&l| l.min(max_block)).sum();
        while sum < total {
            let l = match r.below(8) {
                0 => 0,
                1 => max_block + r.below(4),
                2 => 1 + r.below(3),
                _ => r.below(max_block + 1),
            };
            sum += l.min(max_block);
            lens.push(l);
        }
        lens
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|x| x.to_bits()).collect()
    }

    #[derive(Clone, Copy, Debug)]
    enum Mode {
        /// `upsample` alone, the result compared.
        Up,
        /// `downsample` alone, the buffer filled by the test.
        Down,
        /// Both, the buffer edited in between.
        RoundTrip,
    }

    /// What happens to both oversamplers in the middle of the stream.
    #[derive(Clone, Copy, Debug)]
    enum Event {
        Nothing,
        /// Every 23rd block the factor changes (and so the state is cleared).
        FactorChanges,
        /// Every 29th block `reset` is called.
        Resets,
        /// At every 31st block a clone is made, and the clone continues beside the original.
        Clones,
    }

    const FACTORS: [OversampleFactor; 3] = [
        OversampleFactor::X2,
        OversampleFactor::X4,
        OversampleFactor::X8,
    ];

    /// A cheap non-linearity the caller of the real thing might apply to the buffer. It keeps
    /// every value finite.
    fn edit(v: &mut [f32]) {
        for x in v {
            *x = (*x * 1.5).clamp(-1.0, 1.0) * 0.75 + *x * 0.125;
        }
    }

    /// Runs the new oversampler and the reference on the same stream and the same blocks and
    /// asserts that every output bit, of the upsampled buffer and of the downsampled output, is
    /// the same. After a `Clones` event the clone follows the same blocks and must agree too.
    fn compare(factor: OversampleFactor, max_block: usize, mode: Mode, event: Event, seed: u32) {
        let ctx = format!("{factor:?} max_block {max_block} {mode:?} {event:?}");
        let x = stream(seed, 1);
        let hi_stream = stream(seed ^ 0x55aa, 8);
        let lens = block_lengths(max_block, x.len(), seed);
        let mut new = Oversampler::new(max_block);
        let mut old = reference::Oversampler::new(max_block);
        new.set_factor(factor);
        old.set_factor(factor);
        let mut twin: Option<Oversampler> = None;
        let (mut at, mut at_hi) = (0usize, 0usize);
        for (b, &len) in lens.iter().enumerate() {
            let used = len.min(max_block);
            if at + used > x.len() {
                break;
            }
            match event {
                Event::FactorChanges if b % 23 == 22 => {
                    let f = [
                        OversampleFactor::X2,
                        OversampleFactor::X4,
                        OversampleFactor::X8,
                        OversampleFactor::X1,
                    ][(b / 23) % 4];
                    new.set_factor(f);
                    old.set_factor(f);
                    twin = None;
                }
                Event::Resets if b % 29 == 28 => {
                    new.reset();
                    old.reset();
                    twin = None;
                }
                Event::Clones if b % 31 == 30 => twin = Some(new.clone()),
                _ => {}
            }
            let r = new.factor().ratio();
            let input = &x[at..(at + len).min(x.len())];
            let (mut out_new, mut out_old, mut out_twin) =
                (vec![0.0f32; len], vec![0.0f32; len], vec![0.0f32; len]);
            let at_block = format!("{ctx}: block {b} len {len}");
            match mode {
                Mode::Up => {
                    let want = old.upsample(input).to_vec();
                    assert_eq!(bits(new.upsample(input)), bits(&want), "{at_block}");
                    if let Some(t) = twin.as_mut() {
                        assert_eq!(bits(t.upsample(input)), bits(&want), "{at_block}: clone");
                    }
                }
                Mode::Down => {
                    let fill = &hi_stream[at_hi..at_hi + used * r];
                    at_hi += used * r;
                    // The test fills the buffer itself; the up filters only see zeros.
                    let zeros = vec![0.0f32; len];
                    new.upsample(&zeros).copy_from_slice(fill);
                    old.upsample(&zeros).copy_from_slice(fill);
                    new.downsample(&mut out_new);
                    old.downsample(&mut out_old);
                    assert_eq!(bits(&out_new), bits(&out_old), "{at_block}");
                    if let Some(t) = twin.as_mut() {
                        t.upsample(&zeros).copy_from_slice(fill);
                        t.downsample(&mut out_twin);
                        assert_eq!(bits(&out_twin), bits(&out_old), "{at_block}: clone");
                    }
                }
                Mode::RoundTrip => {
                    let hi_new = new.upsample(input);
                    let hi_old = old.upsample(input);
                    assert_eq!(bits(hi_new), bits(hi_old), "{at_block}: up");
                    edit(hi_new);
                    edit(hi_old);
                    new.downsample(&mut out_new);
                    old.downsample(&mut out_old);
                    assert_eq!(bits(&out_new), bits(&out_old), "{at_block}");
                    if let Some(t) = twin.as_mut() {
                        edit(t.upsample(input));
                        t.downsample(&mut out_twin);
                        assert_eq!(bits(&out_twin), bits(&out_old), "{at_block}: clone");
                    }
                }
            }
            at += used;
        }
    }

    #[test]
    fn upsample_alone_is_bit_exact() {
        for factor in FACTORS {
            for max_block in [1, 7, 64, 512] {
                compare(factor, max_block, Mode::Up, Event::Nothing, 11);
            }
        }
    }

    #[test]
    fn downsample_alone_is_bit_exact() {
        for factor in FACTORS {
            for max_block in [1, 7, 64, 512] {
                compare(factor, max_block, Mode::Down, Event::Nothing, 12);
            }
        }
    }

    #[test]
    fn round_trip_is_bit_exact() {
        for factor in FACTORS {
            for max_block in [1, 7, 64, 512] {
                compare(factor, max_block, Mode::RoundTrip, Event::Nothing, 13);
            }
        }
    }

    #[test]
    fn state_operations_are_bit_exact() {
        for event in [Event::FactorChanges, Event::Resets, Event::Clones] {
            for mode in [Mode::Up, Mode::Down, Mode::RoundTrip] {
                for max_block in [7, 64, 512] {
                    compare(OversampleFactor::X4, max_block, mode, event, 14);
                }
                compare(OversampleFactor::X8, 64, mode, event, 15);
                compare(OversampleFactor::X2, 64, mode, event, 16);
            }
        }
    }

    #[test]
    fn negative_zero_input_gives_positive_zero() {
        // `0.0 + (c * -0.0)` is `+0.0`: the first tap is an addition like every other.
        for factor in FACTORS {
            let mut os = Oversampler::new(16);
            os.set_factor(factor);
            let hi = os.upsample(&[-0.0; 16]);
            assert!(hi.iter().all(|v| v.to_bits() == 0), "{factor:?} up");
            let mut out = [-1.0f32; 16];
            os.downsample(&mut out);
            assert!(out.iter().all(|v| v.to_bits() == 0), "{factor:?} down");
        }
    }

    /// NaN and infinities in the input: whether an output is NaN and the bits of every output
    /// that is not NaN must match the reference. The payload of a NaN is not something this
    /// project promises, so it is not compared.
    #[test]
    fn non_finite_input_matches_where_it_is_defined() {
        let mut r = Rng(21);
        let mut x: Vec<f32> = (0..3000).map(|_| 0.8 * r.unit()).collect();
        for (i, v) in [
            (100, f32::NAN),
            (101, f32::INFINITY),
            (900, f32::NEG_INFINITY),
            (901, f32::INFINITY),
            (1500, f32::INFINITY),
            (2000, -f32::NAN),
            (2700, f32::NEG_INFINITY),
        ] {
            x[i] = v;
        }
        let same = |a: &[f32], b: &[f32], what: &str| {
            assert_eq!(a.len(), b.len());
            for (i, (p, q)) in a.iter().zip(b).enumerate() {
                assert_eq!(p.is_nan(), q.is_nan(), "{what} {i}");
                if !p.is_nan() {
                    assert_eq!(p.to_bits(), q.to_bits(), "{what} {i}");
                }
            }
        };
        for factor in FACTORS {
            for max_block in [7, 64] {
                // One pair upsamples the stream, another downsamples what the first produced.
                let mut up_new = Oversampler::new(max_block);
                let mut up_old = reference::Oversampler::new(max_block);
                let mut down_new = Oversampler::new(max_block);
                let mut down_old = reference::Oversampler::new(max_block);
                for o in [&mut up_new, &mut down_new] {
                    o.set_factor(factor);
                }
                up_old.set_factor(factor);
                down_old.set_factor(factor);
                for chunk in x.chunks(max_block) {
                    let hi = up_old.upsample(chunk).to_vec();
                    same(up_new.upsample(chunk), &hi, "up");
                    let zeros = vec![0.0; chunk.len()];
                    down_new.upsample(&zeros).copy_from_slice(&hi);
                    down_old.upsample(&zeros).copy_from_slice(&hi);
                    let (mut a, mut b) = (vec![0.0; chunk.len()], vec![0.0; chunk.len()]);
                    down_new.downsample(&mut a);
                    down_old.downsample(&mut b);
                    same(&a, &b, "down");
                }
            }
        }
    }

    #[test]
    fn working_memory_is_bounded() {
        // The working buffers do not grow with `max_block` (only the buffer the caller edits does).
        let heap = |os: &Oversampler| -> usize {
            4 * (os.buf.len()
                + os.scratch.len()
                + [&os.up1, &os.up2, &os.up3]
                    .iter()
                    .map(|u| u.lin.len())
                    .sum::<usize>()
                + [&os.down1, &os.down2, &os.down3]
                    .iter()
                    .map(|d| d.lin_a.len() + d.lin_b.len())
                    .sum::<usize>())
        };
        let (small, large) = (Oversampler::new(64), Oversampler::new(4096));
        println!(
            "size_of {} B, heap {} B (64) / {} B (4096)",
            std::mem::size_of::<Oversampler>(),
            heap(&small),
            heap(&large)
        );
        assert_eq!(heap(&large) - heap(&small), 4 * 8 * (4096 - 64));
    }
}
