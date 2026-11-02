//! Partitioned convolution with zero added latency.
//!
//! One [`Convolver`] convolves one channel with an impulse response (IR) of up to a fixed length.
//! The IR is split at [`PARTITION`] samples:
//!
//! * the **head** `h[0..P)` is convolved directly in the time domain, so the output at sample `n`
//!   already contains the input at sample `n` (no latency);
//! * the **tail** `h[P..)` is cut into uniform partitions of `P` samples, each transformed once to
//!   `2P`-point spectra. Every `P` input samples the input block is transformed, pushed into a
//!   frequency-domain delay line, multiplied with the partition spectra and summed (the standard
//!   uniform-partition overlap-save scheme). The result is the tail's contribution to the *next*
//!   block, which is exactly when it is needed because the tail starts `P` samples late.
//!
//! The output equals the direct convolution to float rounding. The sum order is fixed (8 lane
//! dot product for the head, partition order for the tail), so results are bit-identical for any
//! host block size.
//!
//! # No allocation after construction
//!
//! [`Convolver::new`] allocates two IR slots and the delay line for the longest IR. Loading an IR
//! never allocates: [`Convolver::begin_build`] copies the head and starts a build,
//! [`Convolver::build_step`] transforms a few partitions per call (a device calls it once per
//! batch), so a multi-second IR costs a small slice of each batch for a few hundred milliseconds
//! instead of one long stall.
//!
//! # Swapping the IR without a click
//!
//! * On a fresh convolver (nothing processed since the last [`Convolver::reset`]) the IR is built
//!   into the active slot and is usable while it builds: partition `k` is ready long before
//!   the input reaches it, so the result is exact.
//! * Otherwise the new IR is built into the spare slot while the old one keeps sounding; when it
//!   is complete, the next block is the linear crossfade of both convolutions (computed on the
//!   same input history), after which the new IR is the active one.

use crate::spectral::Fft;

/// Partition length in samples (a multiple of the 64-sample batch and of 8).
pub const PARTITION: usize = 256;
const N: usize = 2 * PARTITION;
/// Bins kept per spectrum (`0..=P`, the rest is the conjugate mirror).
const BINS: usize = PARTITION + 1;
/// Length of the fade-out applied when an IR is cut at the longest supported length.
const CUT_FADE: usize = 1_024;

/// One IR: the reversed head and the partition spectra.
#[derive(Debug, Clone)]
struct Slot {
    /// `h[P-1-j]` for `j` in `0..P` (zero past the IR end).
    head: Vec<f32>,
    g_re: Vec<f32>,
    g_im: Vec<f32>,
    /// Partitions built so far (the tail loop reads only these).
    parts: usize,
}

impl Slot {
    fn new(cap: usize) -> Self {
        Self {
            head: vec![0.0; PARTITION],
            g_re: vec![0.0; cap * BINS],
            g_im: vec![0.0; cap * BINS],
            parts: 0,
        }
    }
}

/// A build in progress.
#[derive(Debug, Clone, Copy)]
struct Build {
    /// IR length used (after the cap), in samples.
    len: usize,
    /// Whether the IR was cut at the cap (fade the end).
    cut: bool,
    /// Built into the active slot (a fresh convolver) rather than the spare one.
    direct: bool,
    gain: f32,
    total: usize,
}

/// A one-channel zero-latency partitioned convolver.
#[derive(Debug, Clone)]
pub struct Convolver {
    cap: usize,
    fft: Fft,
    slots: [Slot; 2],
    active: usize,
    /// Doubled ring of the last `P` input samples.
    ring: Vec<f32>,
    pos: usize,
    /// `[previous block | current block]`.
    xbuf: Vec<f32>,
    fill: usize,
    fdl_re: Vec<f32>,
    fdl_im: Vec<f32>,
    newest: usize,
    tails: [Vec<f32>; 2],
    re: Vec<f32>,
    im: Vec<f32>,
    acc_re: Vec<f32>,
    acc_im: Vec<f32>,
    building: Option<Build>,
    built: usize,
    /// The spare slot is complete and waits for the next block boundary.
    pending: bool,
    /// The current block crossfades the active slot into the spare one.
    fade: bool,
    fresh: bool,
}

/// Dot product with eight independent lanes (fixed summation order).
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    let (ca, _) = a.as_chunks::<8>();
    let (cb, _) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for l in 0..8 {
            acc[l] += x[l] * y[l];
        }
    }
    ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]))
}

impl Convolver {
    /// A convolver for IRs of up to `max_ir` samples (at least one partition). Non-RT.
    pub fn new(max_ir: usize) -> Self {
        let cap = max_ir.saturating_sub(PARTITION).div_ceil(PARTITION).max(1);
        Self {
            cap,
            fft: Fft::new(N),
            slots: [Slot::new(cap), Slot::new(cap)],
            active: 0,
            ring: vec![0.0; 2 * PARTITION],
            pos: 0,
            xbuf: vec![0.0; N],
            fill: 0,
            fdl_re: vec![0.0; cap * BINS],
            fdl_im: vec![0.0; cap * BINS],
            newest: 0,
            tails: [vec![0.0; PARTITION], vec![0.0; PARTITION]],
            re: vec![0.0; N],
            im: vec![0.0; N],
            acc_re: vec![0.0; BINS],
            acc_im: vec![0.0; BINS],
            building: None,
            built: 0,
            pending: false,
            fade: false,
            fresh: true,
        }
    }

    /// The longest IR in samples this convolver holds (longer ones are cut with a fade).
    pub fn max_ir(&self) -> usize {
        (self.cap + 1) * PARTITION
    }

    /// Latency in samples: always zero (the head is convolved directly).
    pub const fn latency_frames(&self) -> u32 {
        0
    }

    /// Clears the input history and any crossfade; the loaded IR stays. The next load installs
    /// directly (no crossfade). Does not allocate.
    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.xbuf.fill(0.0);
        self.fdl_re.fill(0.0);
        self.fdl_im.fill(0.0);
        for t in &mut self.tails {
            t.fill(0.0);
        }
        self.pos = 0;
        self.fill = 0;
        self.newest = 0;
        self.pending = false;
        self.fade = false;
        self.fresh = true;
    }

    /// Whether a crossfade is running (a new build must wait until it is over).
    pub fn fading(&self) -> bool {
        self.fade
    }

    /// Whether a build is in progress (call [`Convolver::build_step`]).
    pub fn building(&self) -> bool {
        self.building.is_some()
    }

    /// Whether the active IR is empty (the wet output is silent).
    pub fn is_empty(&self) -> bool {
        self.slots[self.active].head.iter().all(|v| *v == 0.0) && self.slots[self.active].parts == 0
    }

    /// Starts loading `ir` scaled by `gain` (an empty `ir` clears the IR). Copies the head
    /// right away; the tail is built by [`Convolver::build_step`]. A build in progress is
    /// replaced. The caller must not call this while [`Convolver::fading`] is true. Never
    /// allocates.
    pub fn begin_build(&mut self, ir: &[f32], gain: f32) {
        let direct = self.fresh;
        let target = if direct { self.active } else { 1 - self.active };
        let max = self.max_ir();
        let len = ir.len().min(max);
        let slot = &mut self.slots[target];
        slot.parts = 0;
        slot.head.fill(0.0);
        for (j, v) in ir[..len.min(PARTITION)].iter().enumerate() {
            slot.head[PARTITION - 1 - j] = *v * gain;
        }
        let total = len.saturating_sub(PARTITION).div_ceil(PARTITION);
        self.built = 0;
        self.pending = false;
        self.building = Some(Build {
            len,
            cut: ir.len() > max,
            direct,
            gain,
            total,
        });
        if total == 0 {
            self.finish_build(direct);
        }
    }

    fn finish_build(&mut self, direct: bool) {
        self.building = None;
        if !direct {
            self.pending = true;
        }
    }

    /// Transforms up to `budget` more partitions of `ir` (the same slice given to
    /// [`Convolver::begin_build`]). Returns true once the build is complete. Never allocates.
    pub fn build_step(&mut self, ir: &[f32], budget: usize) -> bool {
        let Some(b) = self.building else {
            return true;
        };
        let direct = b.direct;
        let target = if direct { self.active } else { 1 - self.active };
        for _ in 0..budget {
            if self.built >= b.total {
                break;
            }
            let p = self.built;
            let start = PARTITION + p * PARTITION;
            self.re.fill(0.0);
            self.im.fill(0.0);
            for j in 0..PARTITION {
                let i = start + j;
                if i >= b.len {
                    break;
                }
                let mut v = ir[i] * b.gain;
                if b.cut && i + CUT_FADE > b.len {
                    let t = (b.len - i) as f32 / CUT_FADE as f32;
                    v *= t * t * (3.0 - 2.0 * t);
                }
                self.re[j] = v;
            }
            self.fft.forward(&mut self.re, &mut self.im);
            let slot = &mut self.slots[target];
            slot.g_re[p * BINS..(p + 1) * BINS].copy_from_slice(&self.re[..BINS]);
            slot.g_im[p * BINS..(p + 1) * BINS].copy_from_slice(&self.im[..BINS]);
            slot.parts = p + 1;
            self.built += 1;
        }
        if self.built >= b.total {
            self.finish_build(direct);
            return true;
        }
        false
    }

    /// Processes one input sample and returns the convolved output.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        self.fresh = false;
        self.ring[self.pos] = x;
        self.ring[self.pos + PARTITION] = x;
        self.xbuf[PARTITION + self.fill] = x;
        let win = &self.ring[self.pos + 1..=self.pos + PARTITION];
        let a = self.active;
        let mut y = dot(&self.slots[a].head, win) + self.tails[a][self.fill];
        if self.fade {
            let b = 1 - a;
            let yb = dot(&self.slots[b].head, win) + self.tails[b][self.fill];
            let t = (self.fill + 1) as f32 / PARTITION as f32;
            y += (yb - y) * t;
        }
        self.pos = (self.pos + 1) % PARTITION;
        self.fill += 1;
        if self.fill == PARTITION {
            self.end_block();
        }
        y
    }

    fn end_block(&mut self) {
        if self.fade {
            self.active = 1 - self.active;
            self.fade = false;
        }
        self.re.copy_from_slice(&self.xbuf);
        self.im.fill(0.0);
        self.fft.forward(&mut self.re, &mut self.im);
        self.newest = (self.newest + 1) % self.cap;
        let at = self.newest * BINS;
        self.fdl_re[at..at + BINS].copy_from_slice(&self.re[..BINS]);
        self.fdl_im[at..at + BINS].copy_from_slice(&self.im[..BINS]);
        self.xbuf.copy_within(PARTITION.., 0);
        self.fill = 0;
        let a = self.active;
        self.tail_for(a);
        if self.pending {
            self.tail_for(1 - a);
            self.fade = true;
            self.pending = false;
        }
    }

    /// The tail of slot `s` for the next block, from the input spectra in the delay line.
    fn tail_for(&mut self, s: usize) {
        self.acc_re.fill(0.0);
        self.acc_im.fill(0.0);
        let parts = self.slots[s].parts.min(self.cap);
        for k in 0..parts {
            let xi = (self.newest + self.cap - k) % self.cap * BINS;
            let (xr, xm) = (&self.fdl_re[xi..xi + BINS], &self.fdl_im[xi..xi + BINS]);
            let gi = k * BINS;
            let (gr, gm) = (
                &self.slots[s].g_re[gi..gi + BINS],
                &self.slots[s].g_im[gi..gi + BINS],
            );
            for b in 0..BINS {
                self.acc_re[b] += xr[b] * gr[b] - xm[b] * gm[b];
                self.acc_im[b] += xr[b] * gm[b] + xm[b] * gr[b];
            }
        }
        self.re[..BINS].copy_from_slice(&self.acc_re);
        self.im[..BINS].copy_from_slice(&self.acc_im);
        for b in 1..PARTITION {
            self.re[N - b] = self.acc_re[b];
            self.im[N - b] = -self.acc_im[b];
        }
        self.fft.inverse(&mut self.re, &mut self.im);
        self.tails[s].copy_from_slice(&self.re[PARTITION..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;

    fn noise(seed: u64, n: usize) -> Vec<f32> {
        let mut r = Rng::new(seed);
        (0..n).map(|_| r.next_bipolar()).collect()
    }

    fn direct(x: &[f32], h: &[f32]) -> Vec<f64> {
        (0..x.len())
            .map(|n| {
                (0..h.len().min(n + 1))
                    .map(|k| f64::from(h[k]) * f64::from(x[n - k]))
                    .sum()
            })
            .collect()
    }

    fn loaded(ir: &[f32]) -> Convolver {
        let mut c = Convolver::new(ir.len().max(PARTITION));
        c.begin_build(ir, 1.0);
        while !c.build_step(ir, 4) {}
        c
    }

    #[test]
    fn matches_direct_convolution_for_random_irs() {
        for (seed, len) in [(1u64, 100usize), (2, 256), (3, 257), (4, 1_000), (5, 3_333)] {
            let h = noise(seed, len);
            let x = noise(seed + 100, 6_000);
            let want = direct(&x, &h);
            let mut c = loaded(&h);
            let got: Vec<f32> = x.iter().map(|v| c.tick(*v)).collect();
            let worst = got
                .iter()
                .zip(&want)
                .map(|(g, w)| (f64::from(*g) - w).abs())
                .fold(0.0f64, f64::max);
            assert!(worst < 2e-3, "len {len}: worst error {worst}");
        }
    }

    #[test]
    fn has_no_latency_and_the_impulse_reproduces_the_ir() {
        let h: Vec<f32> = (0..900).map(|i| 1.0 / (1.0 + i as f32)).collect();
        let mut c = loaded(&h);
        assert_eq!(c.latency_frames(), 0);
        let mut out = Vec::new();
        for n in 0..1_200 {
            out.push(c.tick(if n == 0 { 1.0 } else { 0.0 }));
        }
        for (i, v) in h.iter().enumerate() {
            assert!((out[i] - v).abs() < 1e-4, "sample {i}: {} vs {v}", out[i]);
        }
        assert!(out[900..].iter().all(|v| v.abs() < 1e-4));
    }

    #[test]
    fn a_progressive_first_build_is_exact() {
        let h = noise(9, 4_000);
        let x = noise(10, 5_000);
        let want = direct(&x, &h);
        let mut c = Convolver::new(4_096);
        c.begin_build(&h, 1.0);
        let mut got = Vec::new();
        for chunk in x.chunks(64) {
            // One partition per batch: slower than any device, still ready in time.
            c.build_step(&h, 1);
            got.extend(chunk.iter().map(|v| c.tick(*v)));
        }
        let worst = got
            .iter()
            .zip(&want)
            .map(|(g, w)| (f64::from(*g) - w).abs())
            .fold(0.0f64, f64::max);
        assert!(worst < 2e-3, "{worst}");
    }

    #[test]
    fn an_ir_swap_crossfades_without_a_step() {
        let a = noise(11, 2_000);
        let mut b = noise(12, 2_000);
        for v in &mut b {
            *v *= 0.5;
        }
        let x = noise(13, 20_000);
        let mut c = loaded(&a);
        let mut out = Vec::new();
        for (n, v) in x.iter().enumerate() {
            if n == 4_000 {
                assert!(!c.fading());
                c.begin_build(&b, 1.0);
            }
            if n % 64 == 0 && c.building() {
                c.build_step(&b, 8);
            }
            out.push(c.tick(*v));
        }
        // After the fade the output is the second IR's convolution (history is shared).
        let want = direct(&x, &b);
        let tail = out.len() - 2_000..;
        let worst = out[tail.clone()]
            .iter()
            .zip(&want[tail])
            .map(|(g, w)| (f64::from(*g) - w).abs())
            .fold(0.0f64, f64::max);
        assert!(worst < 2e-3, "{worst}");
        // No sample-to-sample jump beyond what the signals themselves make.
        let steps = out.windows(2).map(|w| (w[1] - w[0]).abs());
        let biggest = steps.fold(0.0f32, f32::max);
        let before = out[..4_000]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(biggest < before * 1.5, "{biggest} vs {before}");
    }

    #[test]
    fn clearing_fades_to_silence() {
        let h = noise(14, 1_500);
        let x = noise(15, 8_000);
        let mut c = loaded(&h);
        let mut out = Vec::new();
        for (n, v) in x.iter().enumerate() {
            if n == 2_048 {
                c.begin_build(&[], 0.0);
            }
            out.push(c.tick(*v));
        }
        assert!(out[2_048..2_048 + 600].iter().any(|v| v.abs() > 0.1));
        assert!(out[out.len() - 1_000..].iter().all(|v| v.abs() < 1e-4));
        assert!(c.is_empty() || !c.fading());
    }

    #[test]
    fn the_output_does_not_depend_on_how_the_input_is_chunked() {
        let h = noise(16, 2_500);
        let x = noise(17, 3_000);
        let run = |step: usize| {
            let mut c = loaded(&h);
            let mut out = Vec::new();
            for chunk in x.chunks(step) {
                out.extend(chunk.iter().map(|v| c.tick(*v)));
            }
            out
        };
        assert_eq!(run(64), run(1_000));
    }

    #[test]
    fn long_irs_are_cut_with_a_fade() {
        let h = vec![0.5f32; 10_000];
        let mut c = Convolver::new(2_048);
        c.begin_build(&h, 1.0);
        while !c.build_step(&h, 8) {}
        let mut out = Vec::new();
        for n in 0..6_000 {
            out.push(c.tick(if n == 0 { 1.0 } else { 0.0 }));
        }
        let max = c.max_ir();
        assert!(
            out[max - 2_000..max - 2_000 + 100]
                .iter()
                .all(|v| (*v - 0.5).abs() < 1e-6)
        );
        assert!(out[max..].iter().all(|v| v.abs() < 1e-4));
    }
}
