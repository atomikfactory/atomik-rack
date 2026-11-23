//! Circular delay line read with 4-point Hermite interpolation, for modulated delays.
//!
//! Same convention as [`crate::DelayLine`]: per sample call `read(d)` first, then `write(x)`;
//! `read(d)` is the input from `d` samples ago (`d = 1` is the latest write). Reads are clamped
//! to `2..=max_delay` (Hermite needs one newer neighbour). Written values below 1e-20 are
//! flushed to 0. Only `+ - *` and `floor`, so results are bit-identical across platforms.

use crate::hermite4;

/// Hermite-interpolated fractional delay line.
#[derive(Debug, Clone, PartialEq)]
pub struct ModDelay {
    buf: Vec<f32>,
    write: usize,
}

impl ModDelay {
    /// Allocates a line for delays up to `max_delay` samples (at least 4). Non-RT.
    pub fn new(max_delay: usize) -> Self {
        Self {
            buf: vec![0.0; max_delay.max(4) + 4],
            write: 0,
        }
    }

    /// Longest supported delay in samples.
    #[inline]
    pub fn max_delay(&self) -> usize {
        self.buf.len() - 4
    }

    /// Zeroes the content.
    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
    }

    /// Reads the input from `delay` samples ago (clamped to `2..=max_delay`).
    #[inline]
    pub fn read(&self, delay: f32) -> f32 {
        let max = self.max_delay() as f32;
        let d = if delay.is_nan() {
            2.0
        } else {
            delay.clamp(2.0, max)
        };
        let (whole, i) = crate::floor::split_floor(d);
        let t = d - whole;
        let len = self.buf.len();
        // Ages i-1..=i+2 are all within 1..len-1.
        let at = |age: usize| self.buf[(self.write + len - age) % len];
        hermite4(at(i - 1), at(i), at(i + 1), at(i + 2), t)
    }

    /// Appends one sample.
    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.write] = crate::flush_denormal(x);
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
    fn mod_delay_integer_delay_reproduces_impulse() {
        let mut d = ModDelay::new(200);
        let out: Vec<f32> = (0..300)
            .map(|n| {
                let y = d.read(57.0);
                d.write(if n == 0 { 1.0 } else { 0.0 });
                y
            })
            .collect();
        assert_eq!(out[57], 1.0);
        assert!(out.iter().enumerate().all(|(i, v)| i == 57 || *v == 0.0));
    }

    #[test]
    fn mod_delay_fractional_read_of_a_slow_sine_is_accurate() {
        let mut d = ModDelay::new(100);
        let w = 0.05f64;
        let x: Vec<f32> = (0..400).map(|n| (w * n as f64).sin() as f32).collect();
        for (n, v) in x.iter().enumerate() {
            let y = d.read(20.37);
            if n > 40 {
                let want = (w * (n as f64 - 20.37)).sin();
                assert!((f64::from(y) - want).abs() < 2e-4, "n={n}");
            }
            d.write(*v);
        }
    }

    #[test]
    fn mod_delay_clamps_and_clears() {
        let mut d = ModDelay::new(10);
        for i in 0..50 {
            d.write(i as f32);
        }
        assert_eq!(d.read(1_000.0), d.read(10.0));
        assert_eq!(d.read(0.0), d.read(2.0));
        assert_eq!(d.read(f32::NAN), d.read(2.0));
        d.clear();
        assert_eq!(d.read(5.5), 0.0);
        d.write(1e-30);
        assert_eq!(d.read(2.0).to_bits(), 0);
    }

    /// `ModDelay::read` as it was when it called `floor`.
    fn read_with_floor(line: &ModDelay, delay: f32) -> f32 {
        let max = line.max_delay() as f32;
        let d = if delay.is_nan() {
            2.0
        } else {
            delay.clamp(2.0, max)
        };
        let whole = d.floor();
        let t = d - whole;
        let i = whole as usize;
        let len = line.buf.len();
        let at = |age: usize| line.buf[(line.write + len - age) % len];
        hermite4(at(i - 1), at(i), at(i + 1), at(i + 2), t)
    }

    #[test]
    fn mod_delay_read_gives_the_bits_it_gave_with_floor() {
        for max in [1usize, 4, 9, 100, 4_800, 100_000] {
            let mut line = ModDelay::new(max);
            let mut state = 7u32;
            for _ in 0..3_000 {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                line.write((state >> 8) as f32 / (1u32 << 24) as f32 - 0.5);
            }
            let m = line.max_delay() as f32;
            let mut delays = vec![
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::MAX,
                -1.0,
                -0.0,
                0.0,
                0.5,
                1.0,
                1.999_999_8,
                2.0,
                2.000_000_2,
                2.5,
                m - 0.5,
                m,
                m + 0.5,
                m * 3.0,
            ];
            for _ in 0..2_000 {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                delays.push(1.5 + (state >> 8) as f32 / (1u32 << 24) as f32 * m);
            }
            for d in delays {
                assert_eq!(
                    line.read(d).to_bits(),
                    read_with_floor(&line, d).to_bits(),
                    "max {max} delay {d:e}"
                );
            }
        }
    }
}
