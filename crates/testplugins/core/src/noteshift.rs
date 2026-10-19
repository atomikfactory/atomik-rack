//! The test note effect: transposes notes and emits them.

use crate::{Event, TimedEvent};

/// Transposes by `semitones` (-24 to 24). An Off follows the note it closes even if the shift
/// changed meanwhile.
#[derive(Debug, Clone)]
pub struct NoteShift {
    semitones: i32,
    /// Per input key: the output key of the note it opened.
    open: [Option<u8>; 128],
}

impl NoteShift {
    /// The transposition, stepped, -24 to 24, default 0.
    pub const SEMITONES: u32 = 1;

    /// A shift of 0.
    pub fn new() -> Self {
        Self {
            semitones: 0,
            open: [None; 128],
        }
    }

    /// Sets a param (clamped, rounded; unknown ids and NaN are ignored).
    pub fn set_param(&mut self, id: u32, value: f64) {
        if id == Self::SEMITONES && !value.is_nan() {
            self.semitones = value.clamp(-24.0, 24.0).round() as i32;
        }
    }

    /// A param value (0 for an unknown id).
    pub fn param(&self, id: u32) -> f64 {
        if id == Self::SEMITONES {
            f64::from(self.semitones)
        } else {
            0.0
        }
    }

    /// Forgets every open note (a reset).
    pub fn reset(&mut self) {
        self.open = [None; 128];
    }

    /// Shifts the notes of `events` into `out`, never growing it beyond its capacity (events
    /// that do not fit are dropped). Other events are ignored.
    pub fn process(&mut self, events: &[TimedEvent], out: &mut Vec<TimedEvent>) {
        let mut push = |offset: u16, event: Event| {
            if out.len() < out.capacity() {
                out.push(TimedEvent { offset, event });
            }
        };
        for e in events {
            match e.event {
                Event::NoteOn { key, velocity, id } => {
                    let slot = usize::from(key & 0x7F);
                    if let Some(old) = self.open[slot].take() {
                        push(e.offset, Event::NoteOff { key: old, id });
                    }
                    let shifted = (i32::from(key) + self.semitones).clamp(0, 127) as u8;
                    self.open[slot] = Some(shifted);
                    push(
                        e.offset,
                        Event::NoteOn {
                            key: shifted,
                            velocity,
                            id,
                        },
                    );
                }
                Event::NoteOff { key, id } => {
                    if let Some(shifted) = self.open[usize::from(key & 0x7F)].take() {
                        push(e.offset, Event::NoteOff { key: shifted, id });
                    }
                }
                _ => {}
            }
        }
    }
}

impl Default for NoteShift {
    fn default() -> Self {
        Self::new()
    }
}
