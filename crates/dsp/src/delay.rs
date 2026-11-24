//! Circular delay line with fractional (linear-interpolated) reads.
//!
//! The buffer is allocated only by [`DelayLine::new`], [`DelayLine::with_reserve`] and
//! [`DelayLine::resize`] (non-real-time). [`DelayLine::read`], [`DelayLine::write`],
//! [`DelayLine::clear`] and [`DelayLine::grow`] never allocate.
//!
//! Convention: per sample, call `read(d)` first, then `write(x)`. `read(d)` returns the input
//! from `d` samples ago (`d = 1` is the most recent write). Values written with magnitude below
//! 1e-20 are stored as 0 (denormal flush for feedback loops).

/// Ring length for a line of `max_delay`: a power of two, so the index wraps with a mask (the
/// delays that can be read are unchanged; the extra length is never read).
fn ring_for(max_delay: usize) -> usize {
    (max_delay.max(1) + 2).next_power_of_two()
}

/// Fractional delay line.
#[derive(Debug, Clone)]
pub struct DelayLine {
    /// The whole allocation (zeroed, so the operating system commits its pages when first
    /// written).
    buf: Vec<f32>,
    /// Length of the ring in use (a power of two, `<= buf.len()`); the rest is a reserve for
    /// [`DelayLine::grow`].
    active: usize,
    /// Longest delay that can be read.
    max: usize,
    /// Longest delay [`DelayLine::grow`] can reach.
    reserve: usize,
    write: usize,
    /// How many of the latest writes stored `+0.0`, in a row, while `tracking`. The ring holds
    /// only zeros once this reaches `active`. Not part of the line's identity: it is derived from
    /// the content.
    zero_run: usize,
    /// [`DelayLine::track_zeros`] was called: `write` keeps `zero_run`. Off by default, so a line
    /// that nobody asks pays one predictable branch per write and nothing else.
    tracking: bool,
}

impl PartialEq for DelayLine {
    fn eq(&self, other: &Self) -> bool {
        self.buf == other.buf
            && self.active == other.active
            && self.max == other.max
            && self.reserve == other.reserve
            && self.write == other.write
    }
}

impl DelayLine {
    /// Allocates a line able to delay up to `max_delay` samples (at least 1). Non-RT.
    pub fn new(max_delay: usize) -> Self {
        Self::with_reserve(max_delay, max_delay)
    }

    /// Allocates room for delays up to `reserve` samples but starts as a line of `max_delay`
    /// (the reserve is only touched once [`DelayLine::grow`] puts it to use). Non-RT.
    pub fn with_reserve(max_delay: usize, reserve: usize) -> Self {
        let max = max_delay.max(1);
        let reserve = reserve.max(max);
        Self {
            buf: vec![0.0; ring_for(reserve)],
            active: ring_for(max),
            max,
            reserve,
            write: 0,
            zero_run: ring_for(max),
            tracking: false,
        }
    }

    /// Reallocates for a new maximum delay and clears the content. Non-RT.
    pub fn resize(&mut self, max_delay: usize) {
        *self = Self::new(max_delay);
    }

    /// Longest supported delay in samples (of the part in use).
    #[inline]
    pub fn max_delay(&self) -> usize {
        self.max
    }

    /// Longest delay [`DelayLine::grow`] can reach.
    pub fn reserve(&self) -> usize {
        self.reserve
    }

    /// Puts the reserve to use for delays up to `max_delay` (clamped to the reserve) and
    /// clears the content, so the history starts silent. Never allocates; does nothing when the
    /// line already reaches `max_delay`.
    pub fn grow(&mut self, max_delay: usize) {
        let max = max_delay.min(self.reserve).max(1);
        if max > self.max {
            self.max = max;
            self.active = ring_for(max);
            self.clear();
        }
    }

    /// Zeroes the content without allocating.
    #[inline]
    pub fn clear(&mut self) {
        self.buf[..self.active].fill(0.0);
        self.write = 0;
        self.zero_run = self.active;
    }

    /// Starts keeping the fact [`DelayLine::holds_only_zeros`] answers, from the content as it
    /// is now (non-RT: it looks at the ring once). `write` then counts the zeros it stores, at
    /// the price of a compare and an add a sample; a line that was never asked pays one
    /// predictable branch. A new line (`new`, `resize`) is not tracked.
    pub fn track_zeros(&mut self) {
        self.tracking = true;
        let silent = self.buf[..self.active].iter().all(|x| x.to_bits() == 0);
        self.zero_run = if silent { self.active } else { 0 };
    }

    /// Whether the ring in use holds only `+0.0` (a value flushed to zero by `write` counts as
    /// the zero it was stored as): true once [`DelayLine::track_zeros`] was called and the last
    /// `active` writes stored zeros (or the line was cleared). One compare; a line that is not
    /// tracked never claims it.
    #[inline]
    pub fn holds_only_zeros(&self) -> bool {
        self.tracking && self.zero_run >= self.active
    }

    /// Reads the input from `delay` samples ago (clamped to `1..=max_delay`), linearly interpolated.
    #[inline]
    pub fn read(&self, delay: f32) -> f32 {
        let max = self.max as f32;
        let d = if delay.is_nan() {
            1.0
        } else {
            delay.clamp(1.0, max)
        };
        let (whole, index) = crate::floor::split_floor(d);
        let frac = d - whole;
        let len = self.active;
        let mask = len - 1;
        // `whole + 1 <= max + 1 < len`, so the subtraction below never underflows.
        let i0 = (self.write + len - index) & mask;
        let i1 = i0.wrapping_sub(1) & mask;
        let a = self.buf[i0];
        a + (self.buf[i1] - a) * frac
    }

    /// Appends one sample.
    #[inline]
    pub fn write(&mut self, x: f32) {
        let stored = if x.abs() < 1e-20 { 0.0 } else { x };
        self.buf[self.write] = stored;
        self.write = (self.write + 1) & (self.active - 1);
        if self.tracking {
            // All ones when a zero was stored, so the run grows; zero otherwise, so it restarts
            // (it cannot wrap round in practice: 2^64 writes).
            let zero = usize::from(stored.to_bits() == 0).wrapping_neg();
            self.zero_run = self.zero_run.wrapping_add(1) & zero;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayline_fractional_read_interpolates() {
        let mut d = DelayLine::new(16);
        for i in 0..10 {
            d.write(i as f32);
        }
        assert!((d.read(1.0) - 9.0).abs() < 1e-6);
        assert!((d.read(1.5) - 8.5).abs() < 1e-6);
        assert!((d.read(2.25) - 7.75).abs() < 1e-6);
        assert!((d.read(10.0) - 0.0).abs() < 1e-6);
        assert!((d.read(9.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn delayline_integer_delay_reproduces_impulse() {
        let mut d = DelayLine::new(100);
        let out: Vec<f32> = (0..200)
            .map(|n| {
                let y = d.read(37.0);
                d.write(if n == 0 { 1.0 } else { 0.0 });
                y
            })
            .collect();
        assert!((out[37] - 1.0).abs() < 1e-6);
        assert!(
            out.iter()
                .enumerate()
                .all(|(i, v)| i == 37 || v.abs() < 1e-9)
        );
    }

    #[test]
    fn delayline_wraps_and_clamps() {
        let mut d = DelayLine::new(8);
        assert_eq!(d.max_delay(), 8);
        for i in 0..100 {
            d.write(i as f32);
        }
        assert!((d.read(1.0) - 99.0).abs() < 1e-6);
        assert!((d.read(8.0) - 92.0).abs() < 1e-6);
        assert!((d.read(1_000.0) - 92.0).abs() < 1e-6, "clamped to max");
        assert!((d.read(0.0) - 99.0).abs() < 1e-6, "clamped to 1");
        assert!((d.read(f32::NAN) - 99.0).abs() < 1e-6);
    }

    #[test]
    fn delayline_clear_and_resize() {
        let mut d = DelayLine::new(8);
        d.write(1.0);
        d.clear();
        assert!((1..=8).all(|k| d.read(k as f32).abs() < 1e-12));
        d.resize(1_000);
        assert_eq!(d.max_delay(), 1_000);
        d.write(1e-30);
        assert_eq!(d.read(1.0).to_bits(), 0.0f32.to_bits(), "denormal flush");
    }

    #[test]
    fn a_reserve_is_put_to_use_by_grow_without_changing_the_part_in_use() {
        let mut a = DelayLine::new(16);
        let mut b = DelayLine::with_reserve(16, 1_000);
        assert_eq!((b.max_delay(), b.reserve()), (16, 1_000));
        for i in 0..40 {
            a.write(i as f32);
            b.write(i as f32);
        }
        assert!((1..=16).all(|k| a.read(k as f32).to_bits() == b.read(k as f32).to_bits()));
        // Growing clears the line and reaches the reserve; asking for less does nothing.
        b.grow(8);
        assert_eq!(b.max_delay(), 16);
        b.grow(5_000);
        assert_eq!(b.max_delay(), 1_000);
        assert!((1..=1_000).all(|k| b.read(k as f32).to_bits() == 0));
        b.write(1.0);
        for _ in 0..999 {
            b.write(0.0);
        }
        assert_eq!(b.read(1_000.0).to_bits(), 1.0f32.to_bits());
    }
}

#[cfg(test)]
mod ring_tests {
    use super::*;

    /// The masked ring reads exactly what a plain modulo ring of the old length read.
    #[test]
    fn the_power_of_two_ring_reads_what_the_modulo_ring_did() {
        for max in [1usize, 5, 8, 100, 1_000] {
            let mut d = DelayLine::new(max);
            let len = max + 2;
            let mut old = vec![0.0f32; len];
            let mut w = 0usize;
            for n in 0..3_000usize {
                let x = ((n * 37 % 101) as f32 - 50.0) * 0.013;
                let delay = 1.0 + (n % (max * 4)) as f32 * 0.37;
                let want = {
                    let dd = delay.clamp(1.0, max as f32);
                    let whole = dd.floor();
                    let i0 = (w + len - whole as usize) % len;
                    let i1 = if i0 == 0 { len - 1 } else { i0 - 1 };
                    old[i0] + (old[i1] - old[i0]) * (dd - whole)
                };
                assert_eq!(d.read(delay).to_bits(), want.to_bits(), "max {max} n {n}");
                d.write(x);
                old[w] = if x.abs() < 1e-20 { 0.0 } else { x };
                w = (w + 1) % len;
            }
        }
    }
}

#[cfg(test)]
mod floor_tests {
    use super::*;

    /// `DelayLine::read` as it was when it called `floor`.
    fn read_with_floor(line: &DelayLine, delay: f32) -> f32 {
        let max = line.max as f32;
        let d = if delay.is_nan() {
            1.0
        } else {
            delay.clamp(1.0, max)
        };
        let whole = d.floor();
        let frac = d - whole;
        let len = line.active;
        let mask = len - 1;
        let i0 = (line.write + len - whole as usize) & mask;
        let i1 = i0.wrapping_sub(1) & mask;
        let a = line.buf[i0];
        a + (line.buf[i1] - a) * frac
    }

    /// A line of `max` filled with `count` samples of seeded noise.
    fn noisy(max: usize, count: usize) -> DelayLine {
        let mut line = DelayLine::new(max);
        let mut state = 0x2545_F491u32;
        for _ in 0..count {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            line.write((state >> 8) as f32 / (1u32 << 24) as f32 - 0.5);
        }
        line
    }

    fn delays(max: usize) -> Vec<f32> {
        let m = max as f32;
        let mut v = vec![
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MAX,
            f32::MIN,
            -1.0,
            -0.0,
            0.0,
            f32::MIN_POSITIVE,
            0.5,
            0.999_999_9,
            1.0,
            1.000_000_1,
            1.5,
            2.0,
            2.5,
            m - 1.5,
            m - 1.0,
            m - 0.5,
            m,
            m + 0.5,
            m + 1.0,
            m * 2.0,
            8_388_607.5,
            8_388_608.0,
            8_388_609.0,
            16_777_215.0,
            16_777_216.0,
            16_777_218.0,
        ];
        let mut state = 99u32;
        for _ in 0..2_000 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            v.push(1.0 + (state >> 8) as f32 / (1u32 << 24) as f32 * m.min(2.0e7));
        }
        v
    }

    #[test]
    fn read_gives_the_bits_it_gave_with_floor() {
        // A line of 1, short ones, a second's worth, and ones above what `f32` counts exactly.
        for max in [1usize, 2, 3, 7, 100, 4_800, 48_000, 1 << 20, (1 << 24) + 5] {
            let line = noisy(max, 10_000);
            for d in delays(max) {
                assert_eq!(
                    line.read(d).to_bits(),
                    read_with_floor(&line, d).to_bits(),
                    "max {max} delay {d:e}"
                );
            }
        }
    }

    #[test]
    fn read_follows_a_moving_delay_bit_for_bit() {
        let mut line = DelayLine::new(2_000);
        let mut old = line.clone();
        for n in 0..20_000u32 {
            let x = ((n * 37 % 101) as f32 - 50.0) * 0.013;
            let d = 1.0 + (n as f32 * 0.37).sin().abs() * 1_990.0;
            assert_eq!(
                line.read(d).to_bits(),
                read_with_floor(&old, d).to_bits(),
                "n {n}"
            );
            line.write(x);
            old.write(x);
        }
    }
}

#[cfg(test)]
mod zero_run_tests {
    use super::*;

    fn tracked(max: usize) -> DelayLine {
        let mut d = DelayLine::new(max);
        d.track_zeros();
        d
    }

    /// What `holds_only_zeros` must say, by looking at the ring.
    fn scan(line: &DelayLine) -> bool {
        line.buf[..line.active].iter().all(|x| x.to_bits() == 0)
    }

    #[test]
    fn a_new_or_cleared_line_holds_only_zeros_and_a_write_of_a_value_breaks_it() {
        let mut d = tracked(16);
        assert!(d.holds_only_zeros());
        d.write(0.25);
        assert!(!d.holds_only_zeros());
        d.clear();
        assert!(d.holds_only_zeros());
    }

    #[test]
    fn the_line_is_dry_again_after_exactly_one_ring_of_zeros() {
        let mut d = tracked(16);
        let len = d.active;
        d.write(1.0);
        for k in 0..len - 1 {
            d.write(0.0);
            assert!(!d.holds_only_zeros(), "{k} zeros after the value");
        }
        d.write(0.0);
        assert!(d.holds_only_zeros());
        assert!(scan(&d));
    }

    #[test]
    fn what_write_flushes_counts_as_the_zero_it_is_stored_as_and_other_bits_do_not() {
        let mut d = tracked(4);
        let len = d.active;
        for x in [-0.0f32, 1e-25, -1e-25] {
            d.write(1.0);
            for _ in 0..len {
                d.write(x);
            }
            assert!(d.holds_only_zeros(), "{x:e}");
            assert!(scan(&d), "{x:e}");
        }
        d.write(f32::NAN);
        for _ in 0..len - 1 {
            d.write(0.0);
        }
        assert!(!d.holds_only_zeros(), "a NaN is not a zero");
        d.write(0.0);
        assert!(d.holds_only_zeros());
    }

    #[test]
    fn grow_and_a_clone_keep_the_fact_and_resize_stops_tracking() {
        let mut d = DelayLine::with_reserve(8, 1_000);
        d.track_zeros();
        d.write(1.0);
        d.grow(500);
        assert!(d.holds_only_zeros() && scan(&d), "grow clears the line");
        d.write(2.0);
        let c = d.clone();
        assert!(!c.holds_only_zeros());
        assert_eq!(c, d);
        d.resize(64);
        assert!(
            !d.holds_only_zeros(),
            "a new line is not tracked until asked"
        );
        d.track_zeros();
        assert!(d.holds_only_zeros());
    }

    #[test]
    fn a_line_that_is_not_tracked_never_claims_it_and_tracking_starts_from_the_content() {
        let mut d = DelayLine::new(8);
        assert!(!d.holds_only_zeros(), "nobody asked");
        d.write(1.0);
        d.track_zeros();
        assert!(!d.holds_only_zeros(), "the ring holds a value");
        for _ in 0..d.active {
            d.write(0.0);
        }
        assert!(d.holds_only_zeros() && scan(&d));
        let mut e = DelayLine::new(8);
        e.write(1.0);
        assert!(!e.holds_only_zeros());
        e.clear();
        assert!(!e.holds_only_zeros(), "clearing does not turn tracking on");
    }
    #[test]
    fn the_fact_always_matches_a_scan_of_the_ring() {
        let mut d = tracked(37);
        let mut state = 7u32;
        for n in 0..20_000u32 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            // Bursts of sound separated by silences of different lengths.
            let sounding = (n / 97) % 5 == 0 && state & 3 != 0;
            let x = match (sounding, state >> 28) {
                (true, _) => (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5,
                (false, 0) => -0.0,
                (false, 1) => 1e-30,
                (false, _) => 0.0,
            };
            d.write(x);
            assert_eq!(d.holds_only_zeros(), scan(&d), "n {n}");
        }
    }
}
