//! The peaks of the host: the loudest sample the engine sent to the device and the loudest it
//! took from it, published once per batch for the audio setup panel.
//!
//! They are measured at the very end of the batch (after the click and the preview voice) and
//! at the very start (the host inputs), so a tone played through the preview voice shows even
//! though it bypasses the routing and the master meters. Both are lock-free atomics holding the
//! bits of a non-negative `f32` (their order as integers is their order as numbers, so a
//! `fetch_max` keeps the largest). A read takes the value and starts the next window at zero.

use atomik_core::Batch;
use std::sync::atomic::{AtomicU32, Ordering};

/// What the engine measured since the last read.
#[derive(Debug, Default)]
pub struct IoPeaks {
    input: AtomicU32,
    output: AtomicU32,
}

/// The largest absolute sample of `channels` (NaN counts as silence).
fn peak_of(channels: &[Batch]) -> f32 {
    let mut peak = 0.0_f32;
    for channel in channels {
        for &s in channel {
            let a = s.abs();
            if a > peak {
                peak = a;
            }
        }
    }
    peak
}

impl IoPeaks {
    /// Records the host inputs of one batch (audio thread; no allocation, no lock).
    pub(crate) fn publish_input(&self, inputs: &[Batch]) {
        self.input
            .fetch_max(peak_of(inputs).to_bits(), Ordering::Relaxed);
    }

    /// Records the host outputs of one batch (audio thread; no allocation, no lock).
    pub(crate) fn publish_output(&self, outputs: &[Batch]) {
        self.output
            .fetch_max(peak_of(outputs).to_bits(), Ordering::Relaxed);
    }

    /// The loudest host input sample since the last call (0 when none).
    pub fn take_input(&self) -> f32 {
        f32::from_bits(self.input.swap(0, Ordering::Relaxed))
    }

    /// The loudest host output sample since the last call (0 when none).
    pub fn take_output(&self) -> f32 {
        f32::from_bits(self.output.swap(0, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::BATCH;

    #[test]
    fn a_read_takes_the_largest_peak_and_starts_again() {
        let peaks = IoPeaks::default();
        let mut quiet = [0.0; BATCH];
        quiet[3] = -0.25;
        let mut loud = [0.0; BATCH];
        loud[9] = 0.5;
        peaks.publish_output(&[quiet]);
        peaks.publish_output(&[loud, quiet]);
        peaks.publish_output(&[quiet]);
        assert_eq!(peaks.take_output(), 0.5);
        assert_eq!(peaks.take_output(), 0.0);
        assert_eq!(peaks.take_input(), 0.0);
        peaks.publish_input(&[quiet]);
        assert_eq!(peaks.take_input(), 0.25);
    }

    #[test]
    fn nan_is_silence() {
        let peaks = IoPeaks::default();
        peaks.publish_output(&[[f32::NAN; BATCH]]);
        assert_eq!(peaks.take_output(), 0.0);
    }
}
