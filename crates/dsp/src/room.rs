//! Small-box and large-room reverb on the existing eight-line [`crate::Fdn8`].
//!
//! One engine, two size ranges ([`RoomRange`]):
//!
//! * **Small:** a short, dense, resonant box (speaker cabinet, stereo widening). Line delays of
//!   `0.4..3.2 ms` times a seeded spread of `1..2.3`, in-loop all-pass delays of a few samples,
//!   `rt60` from 20 ms to 1 s.
//! * **Large:** a conventional room or hall. Line delays of `12..72 ms` times the same spread,
//!   all-pass delays of up to 3 ms, `rt60` from 0.2 s to 12 s, and a pre-delay of up to 250 ms.
//!
//! The input is the mono sum (after a 20 Hz DC blocker, so a constant input cannot pump the
//! loop). Each line's feedback gain is `10^(-3 T / rt60)` for its trip time `T` (line plus
//! all-pass delay), so the broadband energy decay time is the requested `rt60`; `damping` adds a
//! one-pole low-pass in the loop (high frequencies die faster). The output taps are two
//! orthogonal sign patterns of the line outputs (left and right decorrelated) scaled by
//! `sqrt(1 - g^2)` of the mean gain, which keeps the wet level of white noise near the input level
//! whatever the decay time. A slow, shallow, seeded modulation of every line delay breaks up
//! ringing. The fdn reaches the small range (its lines accept delays down to two samples) so no
//! separate engine is needed. Output is wet only: mix outside.

use crate::{DelayLine, FDN_LINES, Fdn8, ParabolicLfo, Rng, exp2_det, flush_denormal};

/// Largest pre-delay in ms.
pub const MAX_PRE_DELAY_MS: f32 = 250.0;
/// Depth of the delay modulation in samples.
const MOD_DEPTH: f32 = 0.3;
/// Injection pattern (orthonormal-ish: eight weights of 0.35).
const INJECT_SIGNS: [f32; FDN_LINES] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];
/// Left and right output tap patterns (orthogonal).
const TAP_L: [f32; FDN_LINES] = [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0];
const TAP_R: [f32; FDN_LINES] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
/// `3 log2(10)`.
const THREE_LOG2_10: f32 = 9.965_784;

/// Which size range the engine works in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomRange {
    /// The short, dense, resonant box.
    Small,
    /// The conventional room or hall.
    Large,
}

impl RoomRange {
    /// Shortest and longest `rt60` in seconds.
    pub const fn rt60_limits(self) -> (f32, f32) {
        match self {
            Self::Small => (0.02, 1.0),
            Self::Large => (0.2, 12.0),
        }
    }

    /// Line delay scale in ms at size 0 and the size span in octaves.
    const fn scale(self) -> (f32, f32) {
        match self {
            Self::Small => (0.4, 3.0),
            Self::Large => (12.0, 2.585),
        }
    }
}

/// Small-box / large-room reverb.
#[derive(Debug, Clone)]
pub struct Room {
    range: RoomRange,
    sample_rate: f32,
    fdn: Fdn8,
    pre: DelayLine,
    ratios: [f32; FDN_LINES],
    ap_samples: [f32; FDN_LINES],
    lfos: [ParabolicLfo; FDN_LINES],
    lfos0: [ParabolicLfo; FDN_LINES],
    delays: [f32; FDN_LINES],
    gains: [f32; FDN_LINES],
    norm: f32,
    damp: f32,
    size: f32,
    rt60: f32,
    damping: f32,
    pre_delay: f32,
    dc_x: f32,
    dc_y: f32,
}

impl Room {
    /// A room of `range` seeded by `seed` (non-RT allocation). Defaults: size 0.5, `rt60` at the
    /// geometric middle of the range, damping 0.3, no pre-delay.
    pub fn new(sample_rate: f32, seed: u64, range: RoomRange) -> Self {
        let mut rng = Rng::new(seed);
        let ratios = std::array::from_fn(|i| 1.0 + 1.3 * (i as f32 + rng.next_f32()) / 8.0);
        let (ms0, octaves) = range.scale();
        let max_ms = ms0 * exp2_det(octaves) * 2.3 + 1.0;
        let max_delay = (max_ms * 0.001 * sample_rate).ceil() as usize + 8;
        let ap_ms = |i: usize| match range {
            RoomRange::Small => 0.06 + 0.05 * i as f32,
            RoomRange::Large => 0.5 + 0.35 * i as f32,
        };
        let ap_len: [usize; FDN_LINES] =
            std::array::from_fn(|i| ((ap_ms(i) * 0.001 * sample_rate).round() as usize).max(2));
        let ap_samples = std::array::from_fn(|i| ap_len[i] as f32);
        let lfos = std::array::from_fn(|_| ParabolicLfo::seeded(&mut rng, 0.25, 0.8, sample_rate));
        let (lo, hi) = range.rt60_limits();
        let mut r = Self {
            range,
            sample_rate,
            fdn: Fdn8::new(max_delay, ap_len),
            pre: DelayLine::new((MAX_PRE_DELAY_MS * 0.001 * sample_rate).ceil() as usize + 2),
            ratios,
            ap_samples,
            lfos,
            lfos0: lfos,
            delays: [2.0; FDN_LINES],
            gains: [0.0; FDN_LINES],
            norm: 1.0,
            damp: 0.0,
            size: 0.5,
            rt60: (lo * hi).sqrt(),
            damping: 0.3,
            pre_delay: 0.0,
            dc_x: 0.0,
            dc_y: 0.0,
        };
        r.update();
        r
    }

    /// The range this room works in.
    pub fn range(&self) -> RoomRange {
        self.range
    }

    /// Size `0..=1` (scales the line delays over the range's span).
    pub fn set_size(&mut self, size: f32) {
        if !size.is_nan() {
            self.size = size.clamp(0.0, 1.0);
            self.update();
        }
    }

    /// Decay time in seconds (clamped to the range's limits).
    pub fn set_rt60_s(&mut self, s: f32) {
        if !s.is_nan() {
            let (lo, hi) = self.range.rt60_limits();
            self.rt60 = s.clamp(lo, hi);
            self.update();
        }
    }

    /// Damping `0..=1` (0 is no loop low-pass).
    pub fn set_damping(&mut self, d: f32) {
        if !d.is_nan() {
            self.damping = d.clamp(0.0, 1.0);
            self.update();
        }
    }

    /// Pre-delay in ms (clamped to `0..=250`).
    pub fn set_pre_delay_ms(&mut self, ms: f32) {
        if !ms.is_nan() {
            self.pre_delay = ms.clamp(0.0, MAX_PRE_DELAY_MS) * 0.001 * self.sample_rate;
        }
    }

    /// The line delays in samples for the current size (before modulation).
    pub fn line_delays(&self) -> [f32; FDN_LINES] {
        self.delays
    }

    /// The feedback gain of each line for the current `rt60`.
    pub fn line_gains(&self) -> [f32; FDN_LINES] {
        self.gains
    }

    fn update(&mut self) {
        let (ms0, octaves) = self.range.scale();
        let scale_ms = ms0 * exp2_det(self.size * octaves);
        let mut gsum = 0.0;
        for i in 0..FDN_LINES {
            let d = (scale_ms * self.ratios[i] * 0.001 * self.sample_rate).max(3.0);
            self.delays[i] = d;
            let trip = (d + self.ap_samples[i]) / self.sample_rate;
            self.gains[i] = exp2_det(-THREE_LOG2_10 * trip / self.rt60);
            gsum += self.gains[i];
        }
        let g = gsum / FDN_LINES as f32;
        self.norm = (1.0 - g * g).max(1.0e-4).sqrt();
        self.damp = self.damping * 0.9;
    }

    /// Clears the network and the pre-delay.
    pub fn reset(&mut self) {
        self.fdn.clear();
        self.pre.clear();
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.lfos = self.lfos0;
    }

    /// Processes one stereo sample and returns the wet signal.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let x = 0.5 * (l + r);
        let hp = x - self.dc_x + 0.9974 * self.dc_y;
        self.dc_x = x;
        self.dc_y = flush_denormal(hp);
        let pre = self.pre.read(self.pre_delay.max(1.0));
        self.pre.write(hp);
        let mut delays = self.delays;
        for (d, lfo) in delays.iter_mut().zip(self.lfos.iter_mut()) {
            *d += MOD_DEPTH * lfo.next_sample();
        }
        let inject: [f32; FDN_LINES] = std::array::from_fn(|i| 0.35 * INJECT_SIGNS[i] * pre);
        let out = self.fdn.tick(&delays, &self.gains, self.damp, 0.5, &inject);
        let (mut wl, mut wr) = (0.0, 0.0);
        for i in 0..FDN_LINES {
            wl += TAP_L[i] * out[i];
            wr += TAP_R[i] * out[i];
        }
        (wl * self.norm, wr * self.norm)
    }

    /// Replaces the block with the wet signal (identical to per-sample calls).
    pub fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (x, y) = self.process(*a, *b);
            *a = x;
            *b = y;
        }
    }
}

/// Decay time in seconds from an impulse response by backward (Schroeder) integration: the time
/// the energy curve takes to fall from -5 dB to -25 dB, times three. Test and tooling helper.
pub fn rt60_from_impulse(ir: &[f32], sample_rate: f32) -> f32 {
    let mut energy = vec![0.0f64; ir.len()];
    let mut acc = 0.0f64;
    for (i, v) in ir.iter().enumerate().rev() {
        acc += f64::from(*v) * f64::from(*v);
        energy[i] = acc;
    }
    let total = energy[0].max(1e-30);
    let db_at = |i: usize| 10.0 * (energy[i] / total).max(1e-30).log10();
    let i5 = (0..ir.len()).find(|i| db_at(*i) <= -5.0).unwrap_or(0);
    let i25 = (0..ir.len())
        .find(|i| db_at(*i) <= -25.0)
        .unwrap_or(ir.len() - 1);
    3.0 * (i25 as f32 - i5 as f32) / sample_rate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{assert_chunk_invariant_stereo, noise, rms};

    const SR: f32 = 48_000.0;

    fn impulse_response(room: &mut Room, secs: f32) -> (Vec<f32>, Vec<f32>) {
        let n = (secs * SR) as usize;
        let mut l = vec![0.0f32; n];
        l[0] = 1.0;
        let mut r = l.clone();
        room.process_block(&mut l, &mut r);
        (l, r)
    }

    fn make(range: RoomRange, size: f32, rt60: f32) -> Room {
        let mut r = Room::new(SR, 7, range);
        r.set_size(size);
        r.set_rt60_s(rt60);
        r.set_damping(0.0);
        r
    }

    #[test]
    fn large_range_reaches_its_decay_times() {
        for rt in [0.5f32, 1.5, 4.0] {
            let mut room = make(RoomRange::Large, 0.5, rt);
            let (l, r) = impulse_response(&mut room, rt * 1.3 + 0.5);
            for ch in [&l, &r] {
                let got = rt60_from_impulse(ch, SR);
                assert!(
                    (got / rt - 1.0).abs() < 0.2,
                    "large rt60 {rt}: measured {got}"
                );
            }
        }
    }

    #[test]
    fn small_range_reaches_the_short_box_decay_times_with_tiny_delays() {
        for (size, rt) in [(0.0f32, 0.05f32), (0.3, 0.12), (1.0, 0.4)] {
            let mut room = make(RoomRange::Small, size, rt);
            let d = room.line_delays();
            let shortest = d.iter().cloned().fold(f32::MAX, f32::min);
            assert!(shortest < 0.0035 * SR * 2.3, "{shortest}");
            let (l, _) = impulse_response(&mut room, rt * 2.0 + 0.1);
            let got = rt60_from_impulse(&l, SR);
            assert!(
                (got / rt - 1.0).abs() < 0.3,
                "small rt60 {rt} size {size}: measured {got}"
            );
        }
        // The smallest box really is small: every line under 2 ms.
        let room = make(RoomRange::Small, 0.0, 0.1);
        assert!(room.line_delays().iter().all(|d| *d < 0.002 * SR));
        // And the large range is two orders of magnitude longer.
        let big = make(RoomRange::Large, 1.0, 1.0);
        assert!(big.line_delays().iter().all(|d| *d > 0.05 * SR));
        assert_eq!(big.range(), RoomRange::Large);
    }

    fn window_energy(x: &[f32], win: usize) -> Vec<f64> {
        x.chunks_exact(win)
            .map(|c| c.iter().map(|v| f64::from(*v).powi(2)).sum())
            .collect()
    }

    #[test]
    fn decay_is_stable_energy_falls_monotonically() {
        for (range, size, rt, win) in [
            (RoomRange::Large, 0.5f32, 2.0f32, 4800usize),
            (RoomRange::Large, 1.0, 6.0, 4800),
            (RoomRange::Small, 0.5, 0.3, 480),
        ] {
            let mut room = make(range, size, rt);
            room.set_damping(0.4);
            let (l, _) = impulse_response(&mut room, rt * 1.5);
            let e = window_energy(&l, win);
            for k in 3..e.len() - 1 {
                assert!(
                    e[k + 1] <= e[k] * 1.3 + 1e-12,
                    "{range:?} window {k}: {} -> {}",
                    e[k],
                    e[k + 1]
                );
            }
            let late = e[e.len() - 1];
            assert!(late < e[2] * 0.1, "{range:?}: tail {late} vs {}", e[2]);
        }
    }

    #[test]
    fn damping_shortens_the_high_frequency_decay() {
        let ir = |damping: f32| {
            let mut room = make(RoomRange::Large, 0.5, 2.0);
            room.set_damping(damping);
            impulse_response(&mut room, 3.0).0
        };
        let dry = ir(0.0);
        let damped = ir(0.9);
        let tail = |x: &[f32]| window_energy(&x[48_000..96_000], 48_000)[0];
        assert!(
            tail(&damped) < tail(&dry) * 0.5,
            "{} vs {}",
            tail(&damped),
            tail(&dry)
        );
    }

    #[test]
    fn pre_delay_holds_the_response_back() {
        let mut room = make(RoomRange::Large, 0.3, 1.0);
        room.set_pre_delay_ms(100.0);
        let (l, _) = impulse_response(&mut room, 0.5);
        assert!(
            l[..4800].iter().all(|v| *v == 0.0),
            "silent for the pre-delay"
        );
        assert!(l[4800..9600].iter().any(|v| v.abs() > 1e-4));
        room.set_pre_delay_ms(f32::NAN);
        room.set_pre_delay_ms(1.0e6);
    }

    #[test]
    fn wet_level_of_noise_is_near_the_input_level_and_channels_decorrelate() {
        for (range, rt) in [
            (RoomRange::Large, 0.5f32),
            (RoomRange::Large, 6.0),
            (RoomRange::Small, 0.1),
        ] {
            let mut room = make(range, 0.5, rt);
            let mut l = noise(1, 96_000, 1.0);
            let mut r = l.clone();
            let input = rms(&l);
            room.process_block(&mut l, &mut r);
            let out = rms(&l[48_000..]);
            assert!(
                out > input * 0.4 && out < input * 2.5,
                "{range:?} {rt}: {out} vs {input}"
            );
            let corr: f64 = l[48_000..]
                .iter()
                .zip(&r[48_000..])
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum::<f64>()
                / (l[48_000..]
                    .iter()
                    .map(|v| f64::from(*v).powi(2))
                    .sum::<f64>());
            assert!(corr.abs() < 0.3, "channels correlated: {corr}");
        }
    }

    #[test]
    fn same_seed_is_identical_other_seed_differs_and_reset_restarts() {
        let run = |seed: u64| {
            let mut room = Room::new(SR, seed, RoomRange::Small);
            impulse_response(&mut room, 0.2).0
        };
        assert_eq!(run(3), run(3));
        assert_ne!(run(3), run(4));
        let mut room = Room::new(SR, 3, RoomRange::Large);
        let a = impulse_response(&mut room, 0.5).0;
        room.reset();
        // Reset restores the whole start state, modulation included: a second render is identical.
        let b = impulse_response(&mut room, 0.5).0;
        assert_eq!(a, b);
    }

    #[test]
    fn setters_clamp_and_ignore_nan() {
        let mut room = Room::new(SR, 1, RoomRange::Small);
        room.set_size(f32::NAN);
        room.set_rt60_s(f32::NAN);
        room.set_damping(f32::NAN);
        room.set_rt60_s(1000.0);
        assert!(room.line_gains().iter().all(|g| *g < 1.0));
        room.set_rt60_s(0.0);
        room.set_size(9.0);
        room.set_damping(9.0);
        let (a, b) = room.process(1.0, 1.0);
        assert!(a.is_finite() && b.is_finite());
        assert_eq!(RoomRange::Small.rt60_limits().0, 0.02);
    }

    #[test]
    fn block_processing_is_chunk_invariant() {
        let a = noise(4, 3_000, 0.7);
        let b = noise(5, 3_000, 0.7);
        assert_chunk_invariant_stereo(
            || {
                let mut r = Room::new(SR, 2, RoomRange::Large);
                r.set_pre_delay_ms(7.0);
                r
            },
            |r, x, y| r.process_block(x, y),
            &a,
            &b,
        );
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_parameters(
            large in proptest::bool::ANY, size in 0.0f32..1.0, rt in 0.0f32..15.0,
            damp in 0.0f32..1.0, pre in 0.0f32..300.0, seed in 0u64..300
        ) {
            let range = if large { RoomRange::Large } else { RoomRange::Small };
            let mut room = Room::new(SR, seed, range);
            room.set_size(size);
            room.set_rt60_s(rt);
            room.set_damping(damp);
            room.set_pre_delay_ms(pre);
            let mut l = noise(seed, 4096, 1.0);
            let mut r = noise(seed + 9, 4096, 1.0);
            room.process_block(&mut l, &mut r);
            proptest::prop_assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 60.0));
        }
    }
}
