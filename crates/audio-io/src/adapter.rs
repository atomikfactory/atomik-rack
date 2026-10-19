//! The device-free core of the audio callback.

use atomik_engine::{Engine, MAX_INTERFACE_CHANNELS};

/// Most channels handed to `Engine::render`; the engine never has more (extra host outputs
/// are zero-filled, extra host inputs ignored).
const MAX_RENDER_CHANNELS: usize = MAX_INTERFACE_CHANNELS as usize;

/// Device-free core of the audio callback: converts between the host's interleaved
/// buffers and the engine's planar channels using buffers preallocated for `max_frames`.
pub struct CallbackAdapter {
    inputs: usize,
    outputs: usize,
    max_frames: usize,
    /// `min(inputs, 32)` planes of `max_frames` samples, contiguous.
    in_planar: Vec<f32>,
    /// `min(outputs, 32)` planes of `max_frames` samples, contiguous.
    out_planar: Vec<f32>,
}

impl std::fmt::Debug for CallbackAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackAdapter")
            .field("inputs", &self.inputs)
            .field("outputs", &self.outputs)
            .field("max_frames", &self.max_frames)
            .finish_non_exhaustive()
    }
}

impl CallbackAdapter {
    /// Allocates planar scratch for `inputs`/`outputs` channels of up to `max_frames` frames.
    pub fn new(inputs: u16, outputs: u16, max_frames: usize) -> Self {
        let inputs = usize::from(inputs);
        let outputs = usize::from(outputs);
        let max_frames = max_frames.max(1);
        Self {
            inputs,
            outputs,
            max_frames,
            in_planar: vec![0.0; inputs.min(MAX_RENDER_CHANNELS) * max_frames],
            out_planar: vec![0.0; outputs.min(MAX_RENDER_CHANNELS) * max_frames],
        }
    }

    /// Host input channels.
    pub fn inputs(&self) -> u16 {
        u16::try_from(self.inputs).unwrap_or(u16::MAX)
    }

    /// Host output channels.
    pub fn outputs(&self) -> u16 {
        u16::try_from(self.outputs).unwrap_or(u16::MAX)
    }

    /// Largest chunk rendered in one `Engine::render` call.
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Renders one host buffer. `interleaved_in.len()` must be `frames * inputs` and
    /// `interleaved_out.len()` `frames * outputs` with `frames <= max_frames`; larger buffers are
    /// processed in `max_frames` chunks. Never allocates.
    ///
    /// The frame count is taken from the output buffer (or from the input buffer when there are
    /// no outputs). A short input buffer reads as silence; output samples beyond a whole number
    /// of frames are zeroed.
    pub fn process(
        &mut self,
        engine: &mut Engine,
        interleaved_in: &[f32],
        interleaved_out: &mut [f32],
    ) {
        let frames = interleaved_out
            .len()
            .checked_div(self.outputs)
            .or_else(|| interleaved_in.len().checked_div(self.inputs))
            .unwrap_or(0);
        let mut done = 0;
        while done < frames {
            let n = self.max_frames.min(frames - done);
            self.render_chunk(engine, interleaved_in, interleaved_out, done, n);
            done += n;
        }
        if let Some(tail) = interleaved_out.get_mut(frames * self.outputs..) {
            tail.fill(0.0);
        }
    }

    /// Renders frames `start..start + n` of the host buffers (`n <= max_frames`).
    fn render_chunk(
        &mut self,
        engine: &mut Engine,
        input: &[f32],
        output: &mut [f32],
        start: usize,
        n: usize,
    ) {
        let mf = self.max_frames;
        let (ins_n, outs_n) = (
            self.inputs.min(MAX_RENDER_CHANNELS),
            self.outputs.min(MAX_RENDER_CHANNELS),
        );
        // De-interleave (missing samples read as silence).
        for (ch, plane) in self.in_planar.chunks_mut(mf).enumerate() {
            for (f, s) in plane[..n].iter_mut().enumerate() {
                *s = input
                    .get((start + f) * self.inputs + ch)
                    .copied()
                    .unwrap_or(0.0);
            }
        }
        {
            let mut ins: [&[f32]; MAX_RENDER_CHANNELS] = [&[]; MAX_RENDER_CHANNELS];
            for (slot, plane) in ins.iter_mut().zip(self.in_planar.chunks(mf)) {
                *slot = &plane[..n];
            }
            let mut outs: [&mut [f32]; MAX_RENDER_CHANNELS] =
                std::array::from_fn(|_| <&mut [f32]>::default());
            for (slot, plane) in outs.iter_mut().zip(self.out_planar.chunks_mut(mf)) {
                *slot = &mut plane[..n];
            }
            engine.render(&ins[..ins_n], &mut outs[..outs_n]);
        }
        // Re-interleave; host channels beyond the engine's 32 are zero-filled.
        let frame_out = output[start * self.outputs..(start + n) * self.outputs]
            .chunks_exact_mut(self.outputs.max(1));
        for (f, frame) in frame_out.enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                *s = if ch < outs_n {
                    self.out_planar[ch * mf + f]
                } else {
                    0.0
                };
            }
        }
    }
}
