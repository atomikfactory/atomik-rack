//! Stereo feedback echo with a modulated time and a gliding time-jump rule.
//!
//! Each channel is a [`crate::DelayLine`] (linear interpolation, like a tape echo that darkens
//! slightly on fractional times). The base delay time follows its target with a **glide**: it moves
//! towards the target by at most `glide` samples per sample (default 0.25, so a jump bends the
//! pitch of the repeats by at most 25 %). A change smaller than that moves at once (a jump of
//! under one glide step is immediate), so small tweaks and modulation follow exactly while a
//! big jump slides. A sine modulation (`mod_depth_ms` at `mod_rate_hz`, right channel inverted
//! when `invert_right`) is added on top of the glided time, with the line limits `1..=max`.
//!
//! The wet signal is fed back (`feedback`, clamped to `0..=0.97`) with the input into the line
//! (`write(x + fb * wet)`), so repeats of an impulse sit at exact multiples of the time with
//! amplitudes `fb^k` and the output is `(1 - mix) x + mix wet` (`mix = 0` is the exact dry signal).

use crate::det_trig::SineLfo;
use crate::{DelayLine, flush_denormal};

/// Largest feedback.
const MAX_FEEDBACK: f32 = 0.97;

/// Stereo feedback echo.
#[derive(Debug, Clone)]
pub struct Echo {
    sample_rate: f32,
    lines: [DelayLine; 2],
    lfo: SineLfo,
    target: f32,
    current: f32,
    glide: f32,
    feedback: f32,
    mix: f32,
    mod_depth: f32,
    invert_right: bool,
    max_samples: f32,
    /// The bits of every value [`Echo::write_pair`] stored since the last [`Echo::end_batch`],
    /// ORed together: zero when they were all `+0.0`.
    written: u32,
    /// Samples written in a row, up to the end of the last batch, that were all `+0.0`.
    zero_run: usize,
}

impl Echo {
    /// An echo whose lines hold up to `max_ms` milliseconds (non-RT allocation). Defaults: time
    /// 250 ms, feedback 0.3, mix 0.5, no modulation, glide 0.25.
    pub fn new(sample_rate: f32, max_ms: f32) -> Self {
        let max = (max_ms.max(2.0) * 0.001 * sample_rate).ceil() as usize + 1;
        let mut e = Self {
            sample_rate,
            lines: [DelayLine::new(max), DelayLine::new(max)],
            lfo: SineLfo::new(0.0),
            target: 0.0,
            current: 0.0,
            glide: 0.25,
            feedback: 0.3,
            mix: 0.5,
            mod_depth: 0.0,
            invert_right: true,
            max_samples: max as f32,
            written: 0,
            zero_run: 0,
        };
        e.zero_run = e.lines[0].ring_len();
        e.set_time_ms(250.0);
        e.current = e.target;
        e
    }

    /// Delay time in ms (clamped to the line); the glide rule moves the sounding time there.
    pub fn set_time_ms(&mut self, ms: f32) {
        if !ms.is_nan() {
            self.target = (ms.max(0.0) * 0.001 * self.sample_rate).clamp(1.0, self.max_samples);
        }
    }

    /// Jumps the sounding time to the target at once (no glide).
    pub fn snap_time(&mut self) {
        self.current = self.target;
    }

    /// Maximum change of the sounding time in samples per sample (clamped to `0.001..=1`).
    pub fn set_glide(&mut self, samples_per_sample: f32) {
        if !samples_per_sample.is_nan() {
            self.glide = samples_per_sample.clamp(0.001, 1.0);
        }
    }

    /// Feedback (clamped to `0..=0.97`).
    pub fn set_feedback(&mut self, fb: f32) {
        if !fb.is_nan() {
            self.feedback = fb.clamp(0.0, MAX_FEEDBACK);
        }
    }

    /// Dry/wet crossfade `0..=1`.
    pub fn set_mix(&mut self, mix: f32) {
        if !mix.is_nan() {
            self.mix = mix.clamp(0.0, 1.0);
        }
    }

    /// Modulation rate (Hz, clamped to 0..=20) and depth (ms, clamped to 0..=20).
    pub fn set_modulation(&mut self, rate_hz: f32, depth_ms: f32) {
        self.lfo
            .set_rate(rate_hz.clamp(0.0, 20.0), self.sample_rate);
        if !depth_ms.is_nan() {
            self.mod_depth = depth_ms.clamp(0.0, 20.0) * 0.001 * self.sample_rate;
        }
    }

    /// Whether the right channel's modulation is inverted.
    pub fn set_invert_right(&mut self, invert: bool) {
        self.invert_right = invert;
    }

    /// The sounding (glided) base time in samples.
    pub fn current_time_samples(&self) -> f32 {
        self.current
    }

    /// Closes a batch of `frames` calls of [`Echo::write_pair`] for [`Echo::holds_only_zeros`]:
    /// the run of zero writes grows by `frames` if every one of them stored `+0.0`, and starts
    /// again otherwise. A device that asks the question calls it after each batch it writes.
    #[inline]
    pub fn end_batch(&mut self, frames: usize) {
        self.zero_run = if self.written == 0 {
            self.zero_run.saturating_add(frames)
        } else {
            0
        };
        self.written = 0;
    }

    /// Whether both lines hold only `+0.0`: the writes of the batches closed by
    /// [`Echo::end_batch`] since the last non-zero one cover the whole ring, or the lines were
    /// cleared. (Only a batch in which every write stored zero counts, so a batch that held a
    /// value at its start never claims more than it knows.)
    #[inline]
    pub fn holds_only_zeros(&self) -> bool {
        self.zero_run >= self.lines[0].ring_len()
    }

    /// Whether the sounding time sits on the target, bit for bit: the glide then adds `0` every
    /// sample.
    #[inline]
    pub fn time_settled(&self) -> bool {
        self.current.to_bits() == self.target.to_bits()
    }

    /// Moves the modulation phase on by `frames` samples, exactly as that many calls of
    /// [`Echo::read_pair`] would. With [`Echo::time_settled`], lines of zeros and a silent input
    /// that is all they change.
    pub fn skip_silent(&mut self, frames: usize) {
        self.lfo.skip(frames);
    }

    /// Clears the lines and the modulation phase; the sounding time snaps to the target.
    pub fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.written = 0;
        self.zero_run = self.lines[0].ring_len();
        self.lfo.set_phase(0.0);
        self.current = self.target;
    }

    /// Advances the glide and the modulation by one sample and reads both lines at the sounding
    /// time: the repeats of this sample. Every call must be followed by [`Echo::write_pair`]
    /// (that is what [`Echo::process`] does); a device that shapes its own feedback path uses the
    /// two directly.
    #[inline]
    pub fn read_pair(&mut self) -> (f32, f32) {
        let diff = self.target - self.current;
        self.current += diff.clamp(-self.glide, self.glide);
        let s = self.lfo.next_sample() * self.mod_depth;
        let dl = (self.current + s).clamp(1.0, self.max_samples);
        let dr =
            (self.current + if self.invert_right { -s } else { s }).clamp(1.0, self.max_samples);
        (self.lines[0].read(dl), self.lines[1].read(dr))
    }

    /// Writes one stereo sample into the lines (denormals flushed); the counterpart of
    /// [`Echo::read_pair`].
    #[inline]
    pub fn write_pair(&mut self, l: f32, r: f32) {
        let (l, r) = (flush_denormal(l), flush_denormal(r));
        self.lines[0].write(l);
        self.lines[1].write(r);
        self.written |= l.to_bits() | r.to_bits();
    }

    /// Processes one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (wl, wr) = self.read_pair();
        self.write_pair(l + self.feedback * wl, r + self.feedback * wr);
        (l + self.mix * (wl - l), r + self.mix * (wr - r))
    }

    /// Processes a stereo block in place (identical to per-sample calls).
    pub fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (x, y) = self.process(*a, *b);
            *a = x;
            *b = y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{assert_chunk_invariant_stereo, noise};

    const SR: f32 = 48_000.0;

    #[test]
    fn echo_repeats_sit_at_exact_multiples_with_feedback_powers() {
        let mut e = Echo::new(SR, 1000.0);
        e.set_time_ms(100.0); // 4800 samples
        e.snap_time();
        e.set_feedback(0.5);
        e.set_mix(1.0);
        let n = 4800 * 5;
        let mut l = vec![0.0f32; n];
        l[0] = 1.0;
        let mut r = l.clone();
        e.process_block(&mut l, &mut r);
        for k in 1..5usize {
            assert_eq!(l[4800 * k], 0.5f32.powi(k as i32 - 1), "repeat {k}");
            assert_eq!(r[4800 * k], l[4800 * k]);
        }
        assert!(
            l.iter()
                .enumerate()
                .all(|(i, v)| i % 4800 == 0 && i > 0 || *v == 0.0)
        );
    }

    #[test]
    fn mix_zero_is_exact_dry() {
        let mut e = Echo::new(SR, 500.0);
        e.set_mix(0.0);
        e.set_feedback(0.9);
        for v in noise(1, 5_000, 1.0) {
            assert_eq!(e.process(v, v).0.to_bits(), v.to_bits());
        }
    }

    #[test]
    fn a_big_time_jump_glides_at_the_glide_rate_and_a_small_one_follows_at_once() {
        let mut e = Echo::new(SR, 1000.0);
        e.set_glide(0.5);
        e.set_time_ms(100.0);
        e.snap_time();
        e.set_time_ms(200.0);
        let start = e.current_time_samples();
        assert_eq!(start, 4800.0);
        for k in 1..=100 {
            e.process(0.0, 0.0);
            assert_eq!(e.current_time_samples(), start + 0.5 * k as f32, "step {k}");
        }
        for _ in 0..(9600 - 4800) * 2 {
            e.process(0.0, 0.0);
        }
        assert_eq!(e.current_time_samples(), 9600.0, "arrives exactly");
        // A change below one glide step is immediate.
        e.set_time_ms(200.0 + 0.2 / 48.0);
        e.process(0.0, 0.0);
        assert!((e.current_time_samples() - 9600.2).abs() < 1e-3);
    }

    #[test]
    fn modulation_moves_the_repeat_around_the_base_time() {
        let mut e = Echo::new(SR, 500.0);
        e.set_time_ms(50.0);
        e.snap_time();
        e.set_feedback(0.0);
        e.set_mix(1.0);
        e.set_modulation(2.0, 1.0); // +-48 samples
        let n = 48_000;
        let slope = 1.0e-4f32;
        let mut l: Vec<f32> = (0..n).map(|i| i as f32 * slope).collect();
        let mut r = l.clone();
        e.process_block(&mut l, &mut r);
        let d: Vec<f64> = (4000..n)
            .map(|i| i as f64 - f64::from(l[i]) / f64::from(slope))
            .collect();
        let (lo, hi) = d
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        assert!(
            (lo - (2400.0 - 48.0)).abs() < 0.2 && (hi - (2400.0 + 48.0)).abs() < 0.2,
            "{lo} {hi}"
        );
    }

    #[test]
    fn feedback_is_bounded_and_the_tail_ends_at_zero() {
        let mut e = Echo::new(SR, 50.0);
        e.set_feedback(9.0);
        e.set_time_ms(3.0);
        e.set_mix(1.0);
        let peak = noise(2, 96_000, 1.0)
            .iter()
            .map(|v| e.process(*v, *v).0.abs())
            .fold(0.0f32, f32::max);
        assert!(peak < 100.0, "{peak}");
        let mut last = 1.0;
        for _ in 0..(SR as usize * 30) {
            last = e.process(0.0, 0.0).0;
        }
        assert_eq!(last, 0.0);
    }

    #[test]
    fn setters_clamp_and_ignore_nan() {
        let mut e = Echo::new(SR, 100.0);
        e.set_time_ms(f32::NAN);
        e.set_glide(f32::NAN);
        e.set_feedback(f32::NAN);
        e.set_mix(f32::NAN);
        e.set_time_ms(1.0e9);
        e.snap_time();
        assert!(e.current_time_samples() <= 4802.0);
        e.set_modulation(f32::NAN, f32::NAN);
        e.set_invert_right(false);
        let (a, b) = e.process(0.5, 0.5);
        assert!(a.is_finite() && b.is_finite());
        e.reset();
        assert_eq!(e.process(0.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn process_is_chunk_invariant() {
        let a = noise(4, 3_000, 0.7);
        let b = noise(5, 3_000, 0.7);
        assert_chunk_invariant_stereo(
            || {
                let mut e = Echo::new(SR, 100.0);
                e.set_time_ms(7.3);
                e.set_modulation(3.0, 0.2);
                e.set_feedback(0.6);
                e
            },
            |e, x, y| e.process_block(x, y),
            &a,
            &b,
        );
    }

    /// Processes `n` batches of 64 samples of `x` (a constant) and closes each.
    fn run_batches(e: &mut Echo, n: usize, x: f32) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..n {
            for _ in 0..64 {
                out.push(e.process(x, x).0);
            }
            e.end_batch(64);
        }
        out
    }

    #[test]
    fn the_zero_run_claims_the_lines_hold_only_zeros_exactly_when_they_do() {
        let mut e = Echo::new(SR, 50.0);
        e.set_time_ms(3.0);
        e.snap_time();
        e.set_feedback(0.5);
        let ring = e.lines[0].ring_len();
        assert!(e.holds_only_zeros(), "a new echo holds nothing");
        run_batches(&mut e, 1, 0.5);
        assert!(!e.holds_only_zeros());
        let mut batches = 0;
        while !e.holds_only_zeros() {
            run_batches(&mut e, 1, 0.0);
            batches += 1;
            assert!(batches < 1_000);
            // Whenever it claims zeros, a scan of the ring agrees.
            let scan = (1..=e.lines[0].max_delay()).all(|d| {
                e.lines[0].read(d as f32).to_bits() == 0 && e.lines[1].read(d as f32).to_bits() == 0
            });
            assert!(!e.holds_only_zeros() || scan, "batch {batches}");
        }
        assert!(batches * 64 >= ring, "{batches} batches of 64, ring {ring}");
        e.reset();
        run_batches(&mut e, 1, 0.5);
        e.reset();
        assert!(e.holds_only_zeros(), "a reset empties the lines");
    }

    #[test]
    fn skipping_silent_samples_leaves_the_modulation_where_processing_them_does() {
        let make = || {
            let mut e = Echo::new(SR, 50.0);
            e.set_time_ms(3.0);
            e.snap_time();
            e.set_feedback(0.3);
            e.set_modulation(3.0, 1.0);
            e
        };
        let (mut a, mut b) = (make(), make());
        run_batches(&mut a, 2, 0.5);
        run_batches(&mut b, 2, 0.5);
        // Enough silence for the lines to run dry, which is when a skip is allowed.
        run_batches(&mut a, 300, 0.0);
        run_batches(&mut b, 200, 0.0);
        assert!(b.holds_only_zeros() && b.time_settled());
        for _ in 0..100 {
            b.skip_silent(64);
            b.end_batch(64);
        }
        let (ya, yb) = (run_batches(&mut a, 2, 0.4), run_batches(&mut b, 2, 0.4));
        assert!(ya.iter().zip(&yb).all(|(x, y)| x.to_bits() == y.to_bits()));
        assert!(ya.iter().any(|v| v.abs() > 0.1));
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_parameters(
            time in 0.0f32..120.0, fb in 0.0f32..1.0, mix in 0.0f32..1.0,
            rate in 0.0f32..20.0, depth in 0.0f32..20.0, seed in 0u64..300
        ) {
            let mut e = Echo::new(SR, 100.0);
            e.set_time_ms(time);
            e.set_feedback(fb);
            e.set_mix(mix);
            e.set_modulation(rate, depth);
            let mut l = noise(seed, 1024, 1.0);
            let mut r = noise(seed + 9, 1024, 1.0);
            e.process_block(&mut l, &mut r);
            proptest::prop_assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 100.0));
        }
    }
}
