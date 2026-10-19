//! The pure DSP and state of the Atomik Rack test plugins.
//!
//! Both the CLAP test plugins (`atomik-test-clap`) and the VST3 test plugin
//! (`atomik-test-vst3`) are thin format shells over the types of this crate, so the same
//! signal through either format gives the same bits, and the plugin host tests can
//! compare a plugin run through the host against the reference run directly.
//!
//! Everything here is deterministic (no clock, no randomness, no threads), allocation-free in
//! `process`, finite for any input and parameter, and independent of any plugin API: events and
//! buffers are the plain types below. The crate has no dependencies.
//!
//! * [`GainFx`]: a stereo effect with gain, mute, a reported latency (a real delay), a
//!   sidechain ducker and a tail.
//! * [`TestSynth`]: an eight-voice sine or saw instrument that understands pitch bend, CC 1,
//!   channel and per-note pressure, note expression and note ids.
//! * [`NoteShift`]: a note effect that transposes and emits notes.
//! * [`ParamBank`]: 2 000 plain params plus one that can appear at run time.

mod gain;
mod noteshift;
mod params;
mod synth;

pub use gain::GainFx;
pub use noteshift::NoteShift;
pub use params::ParamBank;
pub use synth::TestSynth;

/// Frames in one block: the host always calls with exactly this many.
pub const BLOCK: usize = 64;

/// One block of one channel.
pub type Block = [f32; BLOCK];

/// What a per-note expression changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprKind {
    /// Pitch offset in semitones.
    Tuning,
    /// Pressure, 0 to 1.
    Pressure,
    /// Slide, 0 to 1 (ignored by the test synth).
    Slide,
    /// Volume, 0 to 1.
    Volume,
    /// Pan, -1 to 1 (ignored by the test synth).
    Pan,
}

/// An event for a test plugin. Note ids identify a note across On, Off and expressions; a
/// negative id means "no id: match by key".
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Start a note.
    NoteOn {
        /// MIDI key.
        key: u8,
        /// Velocity in (0, 1].
        velocity: f32,
        /// Note id.
        id: i32,
    },
    /// Release a note.
    NoteOff {
        /// MIDI key.
        key: u8,
        /// Note id.
        id: i32,
    },
    /// Pitch bend, -1 to 1.
    PitchBend(f32),
    /// Continuous controller (123 is all notes off).
    Cc {
        /// Controller number.
        controller: u8,
        /// Value, 0 to 1.
        value: f32,
    },
    /// Channel pressure, 0 to 1.
    ChannelPressure(f32),
    /// Polyphonic pressure on one note.
    PolyPressure {
        /// MIDI key.
        key: u8,
        /// Note id.
        id: i32,
        /// Pressure, 0 to 1.
        value: f32,
    },
    /// A note expression.
    Expr {
        /// MIDI key.
        key: u8,
        /// Note id.
        id: i32,
        /// Which expression.
        kind: ExprKind,
        /// The value.
        value: f32,
    },
    /// A parameter change by the plugin's own id.
    Param {
        /// Param id.
        id: u32,
        /// Plain value.
        value: f64,
    },
}

/// An event at a frame offset inside the block (non-decreasing within a list).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedEvent {
    /// Frame offset, `< BLOCK`.
    pub offset: u16,
    /// The event.
    pub event: Event,
}

/// Maps a normalised value (0 to 1) to the plain range `min..=max`. The VST3 test plugin and the
/// host tests use the same mapping, so a value converted by one is read back exactly by the
/// other. A stepped param is a range of `steps` divisions starting at `min`.
pub fn plain_from_normalized(min: f64, max: f64, normalized: f64) -> f64 {
    min + normalized.clamp(0.0, 1.0) * (max - min)
}

/// The inverse of [`plain_from_normalized`]; an empty range gives 0.
pub fn normalized_from_plain(min: f64, max: f64, plain: f64) -> f64 {
    if max > min {
        ((plain - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Little-endian state helpers: a magic tag followed by `f64` values.
pub(crate) fn encode_state(tag: [u8; 4], values: &[f64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + values.len() * 8);
    out.extend_from_slice(&tag);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// The inverse of [`encode_state`]: exactly `N` finite values after the tag.
pub(crate) fn decode_state<const N: usize>(tag: [u8; 4], bytes: &[u8]) -> Option<[f64; N]> {
    if bytes.len() != 4 + N * 8 || bytes[..4] != tag {
        return None;
    }
    let mut out = [0.0; N];
    let (chunks, _) = bytes[4..].as_chunks::<8>();
    for (i, chunk) in chunks.iter().enumerate() {
        let v = f64::from_le_bytes(*chunk);
        if !v.is_finite() {
            return None;
        }
        out[i] = v;
    }
    Some(out)
}

#[cfg(test)]
mod tests;
