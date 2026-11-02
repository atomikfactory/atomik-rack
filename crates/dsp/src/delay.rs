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
#[derive(Debug, Clone, PartialEq)]
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
        let whole = d.floor();
        let frac = d - whole;
        let len = self.active;
        let mask = len - 1;
        // `whole + 1 <= max + 1 < len`, so the subtraction below never underflows.
        let i0 = (self.write + len - whole as usize) & mask;
        let i1 = i0.wrapping_sub(1) & mask;
        let a = self.buf[i0];
        a + (self.buf[i1] - a) * frac
    }

    /// Appends one sample.
    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.write] = if x.abs() < 1e-20 { 0.0 } else { x };
        self.write = (self.write + 1) & (self.active - 1);
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
