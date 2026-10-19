//! Live MIDI note targeting: the shared [`NoteTarget`] cell the UI thread
//! writes and the MIDI thread reads, and the [`NoteRouter`] that keeps note-offs on the device
//! that got the note-on.

use atomik_core::NoteKind;
use atomik_device_sdk::SocketId;
use atomik_engine::DeviceInstanceId;
use std::sync::atomic::{AtomicU64, Ordering};

/// A notes input: (device instance, notes socket).
pub type Target = (DeviceInstanceId, SocketId);

const PRESENT: u64 = 1 << 48;

/// The current live-notes target, shared lock-free between the UI thread (writer) and the
/// MIDI input thread (reader).
#[derive(Debug, Default)]
pub struct NoteTarget {
    packed: AtomicU64,
}

impl NoteTarget {
    /// No target.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the target.
    ///
    /// Encoding: bit 48 set = present, bits 16..48 the device id, bits 0..16 the socket.
    pub fn set(&self, target: Option<Target>) {
        let packed = target.map_or(0, |(device, socket)| {
            PRESENT | (u64::from(device.0) << 16) | u64::from(socket.0)
        });
        self.packed.store(packed, Ordering::Relaxed);
    }

    /// The target. Lock-free and allocation-free (one atomic load).
    pub fn get(&self) -> Option<Target> {
        let packed = self.packed.load(Ordering::Relaxed);
        (packed & PRESENT != 0).then_some({
            (
                DeviceInstanceId((packed >> 16) as u32),
                SocketId((packed & 0xFFFF) as u16),
            )
        })
    }
}

/// Routes each incoming note to a target.
#[derive(Debug, Clone)]
pub struct NoteRouter {
    held: [Option<Target>; 128],
}

impl Default for NoteRouter {
    fn default() -> Self {
        Self { held: [None; 128] }
    }
}

impl NoteRouter {
    /// A router with no held notes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Routes one note, calling `emit` for each (target, event) to send, in order:
    ///
    /// - a note-on goes to `current` and is remembered per key; if the key is still held on
    ///   another target, that one gets a note-off first;
    /// - a note-off goes where its note-on went (else to `current`);
    /// - all-notes-off goes to `current` and to every other target holding notes, once each.
    ///
    /// Never allocates (it runs on the MIDI input thread).
    pub fn route(
        &mut self,
        kind: NoteKind,
        current: Option<Target>,
        mut emit: impl FnMut(Target, NoteKind),
    ) {
        match kind {
            NoteKind::On { key, .. } => {
                let Some(target) = current else { return };
                if let Some(slot) = self.held.get_mut(usize::from(key))
                    && let Some(old) = slot.replace(target)
                    && old != target
                {
                    emit(old, NoteKind::Off { key });
                }
                emit(target, kind);
            }
            NoteKind::Off { key } => {
                let held = self.held.get_mut(usize::from(key)).and_then(Option::take);
                if let Some(target) = held.or(current) {
                    emit(target, kind);
                }
            }
            NoteKind::AllOff => {
                if let Some(target) = current {
                    emit(target, kind);
                }
                for i in 0..self.held.len() {
                    let Some(target) = self.held[i] else { continue };
                    let seen = Some(target) == current || self.held[..i].contains(&Some(target));
                    if !seen {
                        emit(target, kind);
                    }
                }
                self.held = [None; 128];
            }
            // Channel messages belong to the current target.
            NoteKind::PitchBend { .. } | NoteKind::Cc { .. } | NoteKind::ChannelPressure { .. } => {
                if let Some(target) = current {
                    emit(target, kind);
                }
            }
            // Key messages follow the target that holds the key, else the current one.
            NoteKind::PolyPressure { key, .. } | NoteKind::NoteExpr { key, .. } => {
                let held = self.held.get(usize::from(key)).copied().flatten();
                if let Some(target) = held.or(current) {
                    emit(target, kind);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u32, socket: u16) -> Target {
        (DeviceInstanceId(id), SocketId(socket))
    }

    fn on(key: u8) -> NoteKind {
        NoteKind::On { key, velocity: 0.5 }
    }

    fn off(key: u8) -> NoteKind {
        NoteKind::Off { key }
    }

    fn route(
        r: &mut NoteRouter,
        kind: NoteKind,
        current: Option<Target>,
    ) -> Vec<(Target, NoteKind)> {
        let mut out = Vec::new();
        r.route(kind, current, |target, kind| out.push((target, kind)));
        out
    }

    #[test]
    fn the_cell_round_trips_targets_and_none() {
        let cell = NoteTarget::new();
        assert_eq!(cell.get(), None);
        cell.set(Some(t(7, 3)));
        assert_eq!(cell.get(), Some(t(7, 3)));
        cell.set(Some(t(u32::MAX, u16::MAX)));
        assert_eq!(cell.get(), Some(t(u32::MAX, u16::MAX)));
        cell.set(Some(t(0, 0)));
        assert_eq!(cell.get(), Some(t(0, 0)));
        cell.set(None);
        assert_eq!(cell.get(), None);
    }

    #[test]
    fn notes_go_to_the_current_target() {
        let mut r = NoteRouter::new();
        assert_eq!(
            route(&mut r, on(60), Some(t(1, 0))),
            vec![(t(1, 0), on(60))]
        );
        assert_eq!(
            route(&mut r, off(60), Some(t(1, 0))),
            vec![(t(1, 0), off(60))]
        );
    }

    #[test]
    fn channel_messages_go_to_the_current_target_and_key_messages_to_the_holder() {
        let mut r = NoteRouter::new();
        let bend = NoteKind::PitchBend { value: 0.5 };
        let cc = NoteKind::Cc {
            controller: 1,
            value: 0.3,
        };
        assert_eq!(route(&mut r, bend, Some(t(2, 0))), vec![(t(2, 0), bend)]);
        assert_eq!(route(&mut r, cc, None), vec![]);
        // Key 60 is held on target 1; the keyboard focus has moved to target 2.
        route(&mut r, on(60), Some(t(1, 0)));
        let pp = NoteKind::PolyPressure {
            key: 60,
            value: 0.9,
        };
        assert_eq!(route(&mut r, pp, Some(t(2, 0))), vec![(t(1, 0), pp)]);
        let free = NoteKind::PolyPressure {
            key: 61,
            value: 0.9,
        };
        assert_eq!(route(&mut r, free, Some(t(2, 0))), vec![(t(2, 0), free)]);
    }

    #[test]
    fn without_a_target_notes_are_dropped() {
        let mut r = NoteRouter::new();
        assert_eq!(route(&mut r, on(60), None), vec![]);
        assert_eq!(route(&mut r, off(60), None), vec![]);
    }

    #[test]
    fn a_note_off_follows_its_note_on_after_a_retarget() {
        let mut r = NoteRouter::new();
        route(&mut r, on(60), Some(t(1, 0)));
        assert_eq!(
            route(&mut r, off(60), Some(t(2, 0))),
            vec![(t(1, 0), off(60))]
        );
        // Released: the next off for that key goes to the current target.
        assert_eq!(
            route(&mut r, off(60), Some(t(2, 0))),
            vec![(t(2, 0), off(60))]
        );
    }

    #[test]
    fn a_retriggered_key_on_a_new_target_releases_the_old_one_first() {
        let mut r = NoteRouter::new();
        route(&mut r, on(60), Some(t(1, 0)));
        assert_eq!(
            route(&mut r, on(60), Some(t(2, 0))),
            vec![(t(1, 0), off(60)), (t(2, 0), on(60))]
        );
        assert_eq!(
            route(&mut r, off(60), Some(t(3, 0))),
            vec![(t(2, 0), off(60))]
        );
    }

    #[test]
    fn all_off_reaches_every_target_with_held_notes_once() {
        let mut r = NoteRouter::new();
        route(&mut r, on(60), Some(t(1, 0)));
        route(&mut r, on(61), Some(t(1, 0)));
        route(&mut r, on(62), Some(t(2, 0)));
        let out = route(&mut r, NoteKind::AllOff, Some(t(3, 0)));
        assert_eq!(
            out,
            vec![
                (t(3, 0), NoteKind::AllOff),
                (t(1, 0), NoteKind::AllOff),
                (t(2, 0), NoteKind::AllOff),
            ]
        );
        // Nothing is held any more.
        assert_eq!(
            route(&mut r, off(60), Some(t(3, 0))),
            vec![(t(3, 0), off(60))]
        );
    }

    #[test]
    fn out_of_range_keys_are_passed_through_without_tracking() {
        let mut r = NoteRouter::new();
        assert_eq!(
            route(&mut r, on(200), Some(t(1, 0))),
            vec![(t(1, 0), on(200))]
        );
        assert_eq!(
            route(&mut r, off(200), Some(t(2, 0))),
            vec![(t(2, 0), off(200))]
        );
    }
}
