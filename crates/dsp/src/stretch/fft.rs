//! A radix-2 complex FFT in `f64` with twiddles from the deterministic sine of `det_trig`.

use crate::det_trig::{cos_cycles, sin_cycles};

/// Plan of a power-of-two transform.
#[derive(Debug, Clone)]
pub(crate) struct Fft {
    n: usize,
    rev: Vec<u32>,
    cos: Vec<f64>,
    sin: Vec<f64>,
}

impl Fft {
    pub(crate) fn new(n: usize) -> Self {
        debug_assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        Self {
            n,
            rev: (0..n as u32)
                .map(|i| i.reverse_bits() >> (32 - bits))
                .collect(),
            cos: (0..n / 2)
                .map(|k| cos_cycles(k as f64 / n as f64))
                .collect(),
            sin: (0..n / 2)
                .map(|k| sin_cycles(k as f64 / n as f64))
                .collect(),
        }
    }

    /// In place; `inverse` conjugates the twiddles and divides by `n`.
    pub(crate) fn run(&self, re: &mut [f64], im: &mut [f64], inverse: bool) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sign = if inverse { 1.0 } else { -1.0 };
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            let mut i = 0;
            while i < n {
                for j in 0..half {
                    let (wr, wi) = (self.cos[j * step], sign * self.sin[j * step]);
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
        if inverse {
            let s = 1.0 / n as f64;
            for v in re.iter_mut().chain(im.iter_mut()) {
                *v *= s;
            }
        }
    }
}

/// `atan2(y, x)` from a rational-polynomial arctangent (error below `1e-5` rad) using only
/// IEEE `+ - * /`, so every platform rounds alike.
pub(crate) fn atan2_det(y: f64, x: f64) -> f64 {
    use std::f64::consts::{FRAC_PI_2, PI};
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let (ax, ay) = (x.abs(), y.abs());
    let (num, den, swap) = if ax >= ay {
        (ay, ax, false)
    } else {
        (ax, ay, true)
    };
    let t = num / den;
    let t2 = t * t;
    let mut a = t
        * (0.999_977_26
            + t2 * (-0.332_623_47
                + t2 * (0.193_543_46
                    + t2 * (-0.116_432_87 + t2 * (0.052_653_32 - t2 * 0.011_721_2)))));
    if swap {
        a = FRAC_PI_2 - a;
    }
    if x < 0.0 {
        a = PI - a;
    }
    if y < 0.0 { -a } else { a }
}

/// `sin` and `cos` of an angle in radians, deterministic.
pub(crate) fn sin_cos_det(angle: f64) -> (f64, f64) {
    let c = angle / std::f64::consts::TAU;
    (sin_cycles(c), cos_cycles(c))
}
