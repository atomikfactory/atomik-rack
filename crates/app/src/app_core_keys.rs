//! The Piano Keys strip in the core: its notes are played input, on the same live note path
//! as notes from a MIDI keyboard.
//!
//! A note goes to the notes target (the armed track, else the selected device), and its off
//! goes where its on went even when the target moved while it was held (the
//! [`NoteRouter`] of the MIDI input). [`EngineSink::play_note`] sends it and offers it to the
//! take recorder, which keeps it when its device is armed: a take with the strip is the take
//! a keyboard would have made.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_core::NoteKind;
use atomik_ui::PianoKeysView;

impl<S: EngineSink> AppCore<S> {
    /// The Piano Keys strip.
    pub fn piano_keys(&self) -> &PianoKeysView {
        &self.piano_keys
    }

    /// The Piano Keys strip, mutably (bounds, focus, mode, octave).
    pub fn piano_keys_mut(&mut self) -> &mut PianoKeysView {
        &mut self.piano_keys
    }

    /// Plays (`on`) or releases a key of the Piano Keys strip on the live note path. `velocity`
    /// is the 7-bit value 1..=127.
    pub(super) fn keys_note(&mut self, key: u8, velocity: u8, on: bool) {
        let kind = if on {
            NoteKind::On {
                key,
                velocity: f32::from(velocity.clamp(1, 127)) / 127.0,
            }
        } else {
            NoteKind::Off { key }
        };
        let current = self.notes_target();
        let mut sends = Vec::new();
        self.keys_router
            .route(kind, current, |target, kind| sends.push((target, kind)));
        for (target, kind) in sends {
            let result = self.sink.play_note(target, kind);
            self.engine_result(result);
        }
    }
}
