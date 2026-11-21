//! Whether the core needs the platform loop to run frames at the full rate.
//!
//! The loop runs [`AppCore::frame`] on every wake. While nothing in the core moves, waking
//! sixty times a second only burns the CPU, so the platform asks [`AppCore::wants_fast_frames`]
//! and slows down when the answer is no (see `platform::window_logic`). The answer is true while
//! something here is time-driven or is read from another thread at the frame rate: the
//! transport, a take, a deferred start, a momentary param pulse, a live input analysis, a
//! sampling, the camera, a plugin state capture, live MIDI that was heard in the last second, a
//! control surface, a held MIDI gesture and the audition previews. Everything that only shows a
//! changed value (a meter, a light) is not listed: it paints when it changes, and the platform
//! keeps the full rate for a while after any paint.

use super::*;

/// How long after the last MIDI input the core still wants fast frames (seconds): a knob that
/// is being turned sends messages a few milliseconds apart, and a pause longer than this is the
/// end of the gesture.
pub const MIDI_ACTIVE_SECS: f64 = 1.0;

impl<S: EngineSink> AppCore<S> {
    /// Whether anything in the core needs frames at the full rate at `now_seconds` (the clock
    /// of [`Self::transport_frame`]).
    #[must_use]
    pub fn wants_fast_frames(&self, now_seconds: f64) -> bool {
        self.transport_wants_frames()
            || self.input_wants_frames(now_seconds)
            || self.background_wants_frames()
    }

    /// The transport runs or is about to, a take records, a note is held down by a preview,
    /// or a momentary param waits to be let go.
    fn transport_wants_frames(&self) -> bool {
        self.playing
            || self.take.is_some()
            || self.audio.recording.is_some()
            || self.pending_play.is_some()
            || !self.param_pulses.is_empty()
            || self.player_preview.is_some()
            || self.clip_preview.is_some()
            || self.patch_preview.is_some()
    }

    /// Live input is read, analysed or mapped on the frame: the input analysers, a sampling,
    /// the camera, the control surfaces and live MIDI (heard in the last
    /// [`MIDI_ACTIVE_SECS`], waiting in the hub, or in the middle of a learn or a held
    /// gesture).
    fn input_wants_frames(&self, now_seconds: f64) -> bool {
        self.midi.wants_frames(now_seconds)
            || !self.surfaces.is_empty()
            || self.listen_on()
            || self.trainer_wants_input()
            || self.sampling_device().is_some()
            || self.camera_is_on()
    }

    /// Work that is polled on the frame and has a time of its own: a plugin state capture, the
    /// plugin load queue and a plugin that came back after the engine restarted.
    fn background_wants_frames(&self) -> bool {
        !self.plugins.capturing.is_empty() || !self.plugins_ready()
    }
}

#[cfg(test)]
#[path = "app_core_wake_tests.rs"]
mod tests;
