//! MIDI recording: live notes captured on the MIDI thread with their
//! engine frame, converted to song ticks on the UI thread, and written as note clips.
//!
//! # Design
//!
//! - **Capture.** The MIDI forwarder keeps routing every note live as before. While recording
//!   is armed, each note it sends to an *armed* device is also offered to the [`RecordTap`]:
//!   `(device, event, engine frame E)` goes into a preallocated list behind a short mutex
//!   shared only by the MIDI thread and the UI thread (never the audio thread). The
//!   MIDI thread never allocates there: a full list drops the event and counts it.
//! - **Conversion.** Every UI frame the core drains the tap and converts `E` to a song tick
//!   with [`atomik_engine::SongSnapshot::position_at`] and the latest published
//!   [`TransportState`] (exact in song frames, including loop wraps). Events that arrive
//!   before the engine reports `playing` wait for the first playing state.
//! - **Loop wraps: one clip per pass.** Each loop pass (`loop_count` since the take began) is
//!   its own *lap*. A note still held when the loop wraps is closed at the loop end of its lap;
//!   its later note-off is ignored. On stop, every lap with notes becomes one new clip on the
//!   track's note lane, so each pass lies on top of the previous ones (later clips win,
//!   the older clips stay underneath). Each clip is one `AddClip`, so each lap is one undo
//!   step.
//! - **Clip range.** A lap covers the recorded time (lap 0: from the take's start to the loop
//!   end or the stop; later laps: from the loop start), widened to its notes, rounded out to
//!   whole bars. Notes are relative to the clip start, sorted by `(tick, key)`, without
//!   duplicates, with lengths of at least one tick.
//! - **Input quantize** ([`InputQuantize`]): note starts snap to the nearest 1/8 or 1/16
//!   before the clip is made; lengths are kept.
//! - Notes still held at stop are closed at the stop position.

use atomik_core::{NoteKind, PPQ};
use atomik_engine::{DeviceInstanceId, ScheduledNote, SongPosition, TransportState};
use atomik_model::{Clip, ClipContent, ClipId, DeviceKey, Note};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// Most captured events waiting for the UI thread; more are dropped (and counted).
pub const TAP_CAPACITY: usize = 4096;

/// One captured live note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordedNote {
    /// Engine device that received it.
    pub device: DeviceInstanceId,
    /// `On` or `Off` (an `AllOff` is not recorded).
    pub kind: NoteKind,
    /// Engine frame it was scheduled at (the `ScheduledNote::frame` timeline).
    pub frame: u64,
}

/// Input quantize applied to recorded note starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputQuantize {
    /// Notes keep their played position.
    #[default]
    Off,
    /// Starts snap to 1/8 notes.
    #[serde(rename = "1/8")]
    Eighth,
    /// Starts snap to 1/16 notes.
    #[serde(rename = "1/16")]
    Sixteenth,
}

impl InputQuantize {
    /// The grid in ticks, `None` when off.
    pub fn grid(self) -> Option<i64> {
        match self {
            Self::Off => None,
            Self::Eighth => Some(PPQ / 2),
            Self::Sixteenth => Some(PPQ / 4),
        }
    }

    /// Display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
        }
    }

    /// Every choice, in menu order.
    pub const ALL: [Self; 3] = [Self::Off, Self::Eighth, Self::Sixteenth];
}

#[derive(Debug, Default)]
struct TapInner {
    armed: Vec<DeviceInstanceId>,
    events: Vec<RecordedNote>,
}

/// The capture point between the MIDI thread and the UI thread (module docs).
#[derive(Debug)]
pub struct RecordTap {
    inner: Mutex<TapInner>,
    dropped: AtomicU64,
}

impl Default for RecordTap {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordTap {
    /// A disarmed tap with room for [`TAP_CAPACITY`] events.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TapInner {
                armed: Vec::new(),
                events: Vec::with_capacity(TAP_CAPACITY),
            }),
            dropped: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TapInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Arms the tap for `devices` (UI thread); an empty list disarms it and discards what
    /// was not drained.
    pub fn set_armed(&self, devices: &[DeviceInstanceId]) {
        let mut inner = self.lock();
        inner.armed.clear();
        inner.armed.extend_from_slice(devices);
        if devices.is_empty() {
            inner.events.clear();
        }
    }

    /// Offers a note the forwarder just sent (MIDI thread): kept when its device is armed.
    /// Never allocates.
    pub fn offer(&self, note: &ScheduledNote) {
        if matches!(note.kind, NoteKind::AllOff) {
            return;
        }
        let mut inner = self.lock();
        if !inner.armed.contains(&note.device) {
            return;
        }
        if inner.events.len() < inner.events.capacity() {
            inner.events.push(RecordedNote {
                device: note.device,
                kind: note.kind,
                frame: note.frame,
            });
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Moves the captured events into `out` (UI thread).
    pub fn drain(&self, out: &mut Vec<RecordedNote>) {
        // `append` leaves the list empty with its capacity (the MIDI thread never allocates).
        out.append(&mut self.lock().events);
    }

    /// Events dropped because the list was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// A note being recorded or finished, in song ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TakeNote {
    tick: i64,
    length: i64,
    key: u8,
    velocity: f32,
}

/// One recording pass over the armed tracks (module docs).
#[derive(Debug, Clone)]
pub struct Take {
    tracks: BTreeMap<DeviceInstanceId, DeviceKey>,
    loop_range: Option<(i64, i64)>,
    bar: i64,
    /// `(tick, loop_count)` of the first playing state.
    origin: Option<(i64, u64)>,
    held: BTreeMap<(DeviceInstanceId, u8), (u64, i64, f32)>,
    notes: BTreeMap<(DeviceInstanceId, u64), Vec<TakeNote>>,
    /// Events waiting for a playing transport state.
    pending: Vec<RecordedNote>,
}

impl Take {
    /// A take on `tracks` (engine id → device key), with the song loop range when the loop
    /// is enabled, and `bar` ticks per bar.
    pub fn new(
        tracks: impl IntoIterator<Item = (DeviceInstanceId, DeviceKey)>,
        loop_range: Option<(i64, i64)>,
        bar: i64,
    ) -> Self {
        Self {
            tracks: tracks.into_iter().collect(),
            loop_range: loop_range.filter(|(s, e)| s < e),
            bar: bar.max(1),
            origin: None,
            held: BTreeMap::new(),
            notes: BTreeMap::new(),
            pending: Vec::new(),
        }
    }

    /// The armed devices.
    pub fn devices(&self) -> Vec<DeviceInstanceId> {
        self.tracks.keys().copied().collect()
    }

    /// Where the take started (tick, loop count), once the engine reported playing.
    pub fn origin(&self) -> Option<(i64, u64)> {
        self.origin
    }

    /// Feeds events and the latest transport state. `position` converts an engine frame to
    /// a song position under `state` (the app passes `SongSnapshot::position_at`). Events
    /// wait while the state is not playing.
    pub fn feed(
        &mut self,
        events: &[RecordedNote],
        state: &TransportState,
        position: impl Fn(&TransportState, u64) -> SongPosition,
    ) {
        self.pending.extend_from_slice(events);
        if !state.playing {
            return;
        }
        if self.origin.is_none() {
            self.origin = Some((state.tick, state.loop_count));
        }
        let pending = std::mem::take(&mut self.pending);
        for ev in pending {
            let pos = position(state, ev.frame);
            self.event(ev.device, ev.kind, pos);
        }
    }

    fn lap(&self, loop_count: u64) -> u64 {
        loop_count.saturating_sub(self.origin.map_or(0, |o| o.1))
    }

    /// Closes a held note started in `lap` at song tick `end` (or at the loop end of its
    /// lap when the transport has wrapped since).
    fn close(&mut self, device: DeviceInstanceId, key: u8, now_lap: u64, end: i64) {
        let Some((lap, tick, velocity)) = self.held.remove(&(device, key)) else {
            return;
        };
        let end = match (lap < now_lap, self.loop_range) {
            (true, Some((_, le))) => le,
            _ => end,
        };
        self.notes.entry((device, lap)).or_default().push(TakeNote {
            tick,
            length: (end - tick).max(1),
            key,
            velocity,
        });
    }

    fn event(&mut self, device: DeviceInstanceId, kind: NoteKind, pos: SongPosition) {
        if !self.tracks.contains_key(&device) {
            return;
        }
        let lap = self.lap(pos.loop_count);
        let tick = pos.tick.0.max(0);
        match kind {
            NoteKind::On { key, velocity } => {
                self.close(device, key, lap, tick);
                let velocity = if velocity > 0.0 {
                    velocity.min(1.0)
                } else {
                    1.0
                };
                self.held.insert((device, key), (lap, tick, velocity));
            }
            NoteKind::Off { key } => self.close(device, key, lap, tick),
            NoteKind::AllOff => {
                let keys: Vec<u8> = self
                    .held
                    .keys()
                    .filter(|(d, _)| *d == device)
                    .map(|(_, k)| *k)
                    .collect();
                for key in keys {
                    self.close(device, key, lap, tick);
                }
            }
            // Channel messages and expressions are not recorded into clips.
            _ => {}
        }
    }

    /// Ends the take at `stop` (the transport position when recording stopped): closes held
    /// notes there and returns one clip per (device, lap) with notes, in device then lap
    /// order, ready for `SongCommand::AddClip` on lane 0.
    pub fn finish(mut self, stop: SongPosition, quantize: InputQuantize) -> Vec<(DeviceKey, Clip)> {
        let stop_lap = self.lap(stop.loop_count);
        let held: Vec<(DeviceInstanceId, u8)> = self.held.keys().copied().collect();
        for (device, key) in held {
            self.close(device, key, stop_lap, stop.tick.0.max(0));
        }
        let origin = self.origin.map_or(stop.tick.0, |o| o.0).max(0);
        let mut out = Vec::new();
        for ((device, lap), mut notes) in std::mem::take(&mut self.notes) {
            let Some(key) = self.tracks.get(&device).cloned() else {
                continue;
            };
            if let Some(grid) = quantize.grid() {
                for n in &mut notes {
                    n.tick = (n.tick + grid / 2).div_euclid(grid) * grid;
                }
            }
            notes.sort_by_key(|n| (n.tick, n.key));
            notes.dedup_by(|b, a| (a.tick, a.key) == (b.tick, b.key));
            // The recorded time of this lap.
            let (from, to) = match self.loop_range {
                Some((ls, le)) => {
                    let from = if lap == 0 { origin } else { ls };
                    let to = if lap < stop_lap { le } else { stop.tick.0 };
                    (from, to)
                }
                None => (origin, stop.tick.0),
            };
            let first = notes.iter().map(|n| n.tick).min().unwrap_or(from);
            let last = notes.iter().map(|n| n.tick + n.length).max().unwrap_or(to);
            let start = from.min(first).max(0).div_euclid(self.bar) * self.bar;
            let end = to.max(last).max(start + 1);
            let end = (end + self.bar - 1).div_euclid(self.bar) * self.bar;
            let content = notes
                .iter()
                .map(|n| Note {
                    tick: n.tick - start,
                    length: n.length.max(1),
                    key: n.key,
                    velocity: n.velocity,
                })
                .collect();
            out.push((
                key,
                Clip {
                    id: ClipId(0),
                    start,
                    length: end - start,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    grid: None,
                    content: ClipContent::Notes(content),
                },
            ));
        }
        out
    }
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
