//! The peak of the last few batches, for the level readouts of the analyser devices.
//!
//! A device publishes its display values once per batch (1.3 ms at 48 kHz) and the UI reads them
//! about 60 times a second, so a value that is only the peak of the latest batch would miss most
//! peaks. [`RecentPeak`] keeps the peak of each of the last [`WINDOW`] batches (about 21 ms at
//! 48 kHz, longer than a UI frame) and answers the largest.

use atomik_core::Batch;

/// Batches in the window.
const WINDOW: usize = 16;

/// The largest absolute sample of a batch; a non-finite sample counts as 0.
pub(crate) fn batch_peak(x: &Batch) -> f32 {
    x.iter().fold(
        0.0f32,
        |m, v| if v.is_finite() { m.max(v.abs()) } else { m },
    )
}

/// The peaks of the last [`WINDOW`] batches.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RecentPeak {
    ring: [f32; WINDOW],
    pos: usize,
}

impl RecentPeak {
    /// Forgets everything.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Adds the peak of the batch just processed and returns the largest of the window.
    pub(crate) fn push(&mut self, peak: f32) -> f32 {
        self.ring[self.pos] = peak;
        self.pos = (self.pos + 1) % WINDOW;
        self.ring.iter().fold(0.0f32, |m, v| m.max(*v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_holds_a_peak_for_sixteen_batches() {
        let mut p = RecentPeak::default();
        assert!((p.push(0.5) - 0.5).abs() < 1e-9);
        for _ in 0..15 {
            assert!((p.push(0.0) - 0.5).abs() < 1e-9);
        }
        assert!(p.push(0.0) < 1e-9, "the sixteenth batch after it has gone");
        p.push(0.7);
        p.clear();
        assert!(p.push(0.0) < 1e-9);
    }

    #[test]
    fn the_batch_peak_ignores_the_sign_and_non_finite_samples() {
        let mut b = [0.0f32; 64];
        b[3] = -0.8;
        b[9] = f32::NAN;
        b[10] = f32::INFINITY;
        assert!((batch_peak(&b) - 0.8).abs() < 1e-9);
    }
}
