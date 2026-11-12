//! The One Euro filter: a low-pass filter whose cut-off rises with the speed of the signal, so
//! a still hand is smooth and a fast one has little lag.

use std::f64::consts::PI;

/// The three numbers of the filter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OneEuroParams {
    /// The cut-off in hertz when the signal is still (smaller is smoother).
    pub min_cutoff: f64,
    /// How much the cut-off grows with the speed of the signal (larger is quicker).
    pub beta: f64,
    /// The cut-off for the speed estimate itself.
    pub d_cutoff: f64,
}

/// One filtered signal.
#[derive(Debug, Clone)]
pub struct OneEuro {
    params: OneEuroParams,
    prev: Option<(f64, f64, f64)>,
}

fn alpha(cutoff: f64, dt: f64) -> f64 {
    let tau = 1.0 / (2.0 * PI * cutoff);
    1.0 / (1.0 + tau / dt)
}

impl OneEuro {
    /// A filter with no history.
    pub fn new(params: OneEuroParams) -> Self {
        Self { params, prev: None }
    }

    /// Feeds the sample `x` taken at `t` seconds and returns the filtered value. The first
    /// sample is returned as it is.
    pub fn filter(&mut self, t: f64, x: f64) -> f64 {
        let Some((pt, px, pdx)) = self.prev else {
            self.prev = Some((t, x, 0.0));
            return x;
        };
        let dt = t - pt;
        if dt <= 0.0 {
            return px;
        }
        let dx = (x - px) / dt;
        let dx_hat = pdx + alpha(self.params.d_cutoff, dt) * (dx - pdx);
        let cutoff = self.params.min_cutoff + self.params.beta * dx_hat.abs();
        let x_hat = px + alpha(cutoff, dt) * (x - px);
        self.prev = Some((t, x_hat, dx_hat));
        x_hat
    }
}
