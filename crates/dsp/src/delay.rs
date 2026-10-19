//! Circular delay line with fractional (linear-interpolated) reads.
//!
//! The buffer is allocated only by [`DelayLine::new`] and [`DelayLine::resize`] (non-real-time).
//! [`DelayLine::read`], [`DelayLine::write`] and [`DelayLine::clear`] never allocate.
//!
//! Convention: per sample, call `read(d)` first, then `write(x)`. `read(d)` returns the input
//! from `d` samples ago (`d = 1` is the most recent write). Values written with magnitude below
//! 1e-20 are stored as 0 (denormal flush for feedback loops).

/// Fractional delay line.
#[derive(Debug, Clone, PartialEq)]
pub struct DelayLine {
    buf: Vec<f32>,
    write: usize,
}

impl DelayLine {
    /// Allocates a line able to delay up to `max_delay` samples (at least 1). Non-RT.
    pub fn new(max_delay: usize) -> Self {
        Self {
            buf: vec![0.0; max_delay.max(1) + 2],
            write: 0,
        }
    }

    /// Reallocates for a new maximum delay and clears the content. Non-RT.
    pub fn resize(&mut self, max_delay: usize) {
        *self = Self::new(max_delay);
    }

    /// Longest supported delay in samples.
    #[inline]
    pub fn max_delay(&self) -> usize {
        self.buf.len() - 2
    }

    /// Zeroes the content without allocating.
    #[inline]
    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
    }

    /// Reads the input from `delay` samples ago (clamped to `1..=max_delay`), linearly interpolated.
    #[inline]
    pub fn read(&self, delay: f32) -> f32 {
        let max = self.max_delay() as f32;
        let d = if delay.is_nan() {
            1.0
        } else {
            delay.clamp(1.0, max)
        };
        let whole = d.floor();
        let frac = d - whole;
        let len = self.buf.len();
        // `whole + 1 <= max + 1 < len`, so the subtraction below never underflows.
        let i0 = (self.write + len - whole as usize) % len;
        let i1 = if i0 == 0 { len - 1 } else { i0 - 1 };
        let a = self.buf[i0];
        a + (self.buf[i1] - a) * frac
    }

    /// Appends one sample.
    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.write] = if x.abs() < 1e-20 { 0.0 } else { x };
        self.write += 1;
        if self.write == self.buf.len() {
            self.write = 0;
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
}
