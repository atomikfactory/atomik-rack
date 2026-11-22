//! The ringing effect: copies its input and adds a constant to it while it rings.

use crate::{BLOCK, Block};

/// How much a ringing effect adds to its input (exact in `f32`).
pub const RING_LEVEL: f32 = 0.25;

/// A stereo effect for the sleep rule of the host: `out = in`, plus [`RING_LEVEL`] on both
/// channels for `ring_frames` after the last block that had a non-silent input. What it reports
/// as its tail is up to the shell, so the same DSP is a plugin that tells the truth, one that
/// under-reports and one that reports a tail without end.
#[derive(Debug, Clone)]
pub struct RingFx {
    ring_frames: u32,
    left: u32,
}

impl RingFx {
    /// An effect that rings for `ring_frames` after its input.
    pub fn new(ring_frames: u32) -> Self {
        Self {
            ring_frames,
            left: 0,
        }
    }

    /// Clears the ring.
    pub fn reset(&mut self) {
        self.left = 0;
    }

    /// Processes one block; returns whether the block rings (is not silent because of the
    /// effect itself).
    pub fn process(&mut self, input: &[Block; 2], output: &mut [Block; 2]) -> bool {
        if input
            .iter()
            .any(|b| b.iter().any(|s| s.to_bits() & 0x7FFF_FFFF != 0))
        {
            self.left = self.ring_frames;
        }
        let ringing = self.left > 0;
        let add = if ringing { RING_LEVEL } else { 0.0 };
        for (out, inp) in output.iter_mut().zip(input) {
            for (o, x) in out.iter_mut().zip(inp) {
                *o = x + add;
            }
        }
        self.left = self.left.saturating_sub(BLOCK as u32);
        ringing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_rings_for_the_frames_after_the_input_and_then_is_silent() {
        let mut fx = RingFx::new(128);
        let mut out = [[0.0; BLOCK]; 2];
        let loud = [[0.5; BLOCK]; 2];
        let quiet = [[0.0; BLOCK]; 2];
        assert!(fx.process(&loud, &mut out));
        assert!((out[0][0] - 0.75).abs() < 1e-6);
        assert!(fx.process(&quiet, &mut out));
        assert!((out[1][5] - RING_LEVEL).abs() < 1e-6);
        assert!(!fx.process(&quiet, &mut out));
        assert!(out[0].iter().all(|s| s.abs() < 1e-9));
    }
}
