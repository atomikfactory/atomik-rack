//! MIDI sync in the engine: the clock the player generates for MIDI Clock out
//! ([`SyncOut`], [`clocks_in`]) and the cell it reads to follow an external clock
//! ([`ExternalClock`]).
//!
//! # Clock out
//!
//! The player writes 24 clocks per quarter note, and Start, Stop, Continue and Song Position
//! Pointer on transport changes, into a [`MidiTap`], each stamped with the engine host frame it
//! belongs to. The app's MIDI output thread turns frames into wall-clock times and sends the
//! bytes (see `atomik_midi_io::out`). A clock sits at `F(640 * k)`: the song frame of tick
//! `640 * k` through the tempo map, the same rounding every note gets, so it is exact at a
//! constant tempo and across ramps and does not drift. Everything here is a few atomic stores per
//! message: no allocation, no lock.
//!
//! # Following an external clock
//!
//! A thread of the app that listens to a MIDI port keeps the clock's state in an
//! [`ExternalClock`] (the tempo it measured, whether the sender runs, where it was told to
//! locate); the player reads it at every batch boundary while following. The cell is a seqlock
//! like the transport cell: one writer, any number of readers, nobody waits.

use atomik_core::{PPQ, TempoMap};
use atomik_device_sdk::{MidiEvent, MidiTap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering, fence};

/// Ticks between two MIDI clocks (24 per quarter note).
pub const TICKS_PER_CLOCK: i64 = PPQ / 24;

/// Ticks in one Song Position Pointer unit (a sixteenth note, six clocks).
pub const TICKS_PER_POSITION: i64 = PPQ / 4;

/// The MIDI clock byte.
pub const CLOCK: u8 = 0xF8;
/// The MIDI start byte.
pub const START: u8 = 0xFA;
/// The MIDI continue byte.
pub const CONTINUE: u8 = 0xFB;
/// The MIDI stop byte.
pub const STOP: u8 = 0xFC;
/// The Song Position Pointer status byte.
pub const SONG_POSITION: u8 = 0xF2;

/// The Song Position Pointer message for `tick`: its position in sixteenth notes as 14 bits, the
/// least significant seven bits first (a position past 16383 sixteenths saturates).
pub fn song_position_bytes(tick: i64) -> [u8; 3] {
    let n = (tick.max(0) / TICKS_PER_POSITION).min(0x3FFF) as u16;
    [SONG_POSITION, (n & 0x7F) as u8, (n >> 7) as u8]
}

/// The tick a Song Position Pointer message addresses (`lsb`, `msb`: its two data bytes).
pub fn song_position_ticks(lsb: u8, msb: u8) -> i64 {
    (i64::from(lsb & 0x7F) | i64::from(msb & 0x7F) << 7) * TICKS_PER_POSITION
}

/// What the player sends as MIDI clock (the app sets it through
/// [`crate::EngineHandle::configure_sync_out`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncConfig {
    /// Send clock at all.
    pub enabled: bool,
    /// Keep sending clocks (at the tempo of the position) while the transport is stopped.
    pub while_stopped: bool,
    /// Send Song Position Pointer when the position is set.
    pub song_position: bool,
    /// A locate while playing sends Stop, the position and Continue (Start at the beginning), so
    /// a receiver that needs a stop to move follows; when off it sends only the position.
    pub restart_on_locate: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            while_stopped: false,
            song_position: true,
            restart_on_locate: true,
        }
    }
}

const ENABLED: u32 = 1;
const WHILE_STOPPED: u32 = 2;
const SONG_POSITION_ON: u32 = 4;
const RESTART_ON_LOCATE: u32 = 8;

/// What the player writes and the app reads: the tap with the sync messages and the settings the
/// player reads once per batch.
#[derive(Debug)]
pub struct SyncOut {
    tap: Arc<MidiTap>,
    flags: AtomicU32,
}

impl SyncOut {
    pub(crate) fn new() -> Self {
        Self {
            tap: Arc::new(MidiTap::new()),
            flags: AtomicU32::new(0),
        }
    }

    /// The queue the clock messages go to. One reader.
    pub fn tap(&self) -> Arc<MidiTap> {
        Arc::clone(&self.tap)
    }

    /// Applies `config` (takes effect at the next batch; no queue, never blocks).
    pub fn configure(&self, config: SyncConfig) {
        let mut flags = 0;
        for (on, bit) in [
            (config.enabled, ENABLED),
            (config.while_stopped, WHILE_STOPPED),
            (config.song_position, SONG_POSITION_ON),
            (config.restart_on_locate, RESTART_ON_LOCATE),
        ] {
            if on {
                flags |= bit;
            }
        }
        self.flags.store(flags, Ordering::Relaxed);
    }

    /// The settings in effect.
    pub fn config(&self) -> SyncConfig {
        let f = self.flags.load(Ordering::Relaxed);
        SyncConfig {
            enabled: f & ENABLED != 0,
            while_stopped: f & WHILE_STOPPED != 0,
            song_position: f & SONG_POSITION_ON != 0,
            restart_on_locate: f & RESTART_ON_LOCATE != 0,
        }
    }
}

/// Calls `emit` with the song frame of every MIDI clock that falls in the song frames `[a, b)`:
/// the frames `F(640 * k)` for the whole numbers `k`, through the tempo map `map`. Pure, and no
/// allocation.
pub fn clocks_in(map: TempoMap<'_>, sample_rate: f64, a: u64, b: u64, mut emit: impl FnMut(u64)) {
    if b <= a {
        return;
    }
    let first = (map.tick(a as f64, sample_rate) / TICKS_PER_CLOCK as f64).floor() as i64;
    // One clock back covers the rounding of `F` at the start of the range.
    let mut k = (first - 1).max(0);
    loop {
        let f = map.song_frame(k * TICKS_PER_CLOCK, sample_rate);
        if f >= b {
            break;
        }
        if f >= a {
            emit(f);
        }
        k += 1;
    }
}

/// The player's side of clock out: what the receivers have been told, so that it sends only
/// the changes.
#[derive(Debug)]
pub(crate) struct ClockOut {
    shared: Arc<SyncOut>,
    /// Whether the receivers believe the transport runs.
    running: bool,
    was_enabled: bool,
    /// A locate since the last batch, as a tick.
    located: Option<i64>,
    /// Ticks until the next clock of the free-running clock while stopped.
    idle_to_next: f64,
}

impl ClockOut {
    pub(crate) fn new() -> Self {
        Self {
            shared: Arc::new(SyncOut::new()),
            running: false,
            was_enabled: false,
            located: None,
            idle_to_next: 0.0,
        }
    }

    pub(crate) fn shared(&self) -> Arc<SyncOut> {
        Arc::clone(&self.shared)
    }

    /// A locate to `tick` was applied (sent with the next batch).
    pub(crate) fn located(&mut self, tick: i64) {
        self.located = Some(tick);
    }

    fn send(&self, frame: u64, bytes: &[u8]) {
        if let Some(event) = MidiEvent::new(frame, bytes) {
            // A full queue is counted by the tap; the next message is still sent.
            let _ = self.shared.tap.push(event);
        }
    }

    /// The batch starting at engine frame `frame`: sends Start, Continue or Stop for a change of
    /// the transport since the last batch and the position for a locate. `tick` is the transport
    /// position. Returns the settings in effect (clock off: nothing is sent and nothing is
    /// remembered but the transport state).
    pub(crate) fn batch_start(&mut self, frame: u64, playing: bool, tick: i64) -> SyncConfig {
        let config = self.shared.config();
        let located = self.located.take();
        if !config.enabled {
            self.running = playing;
            self.was_enabled = false;
            return config;
        }
        let at_start = tick <= 0;
        let position = |this: &Self, tick: i64| {
            if config.song_position {
                this.send(frame, &song_position_bytes(tick));
            }
        };
        if !self.was_enabled {
            // Just switched on: tell the receiver where the transport is and whether it runs.
            self.was_enabled = true;
            position(self, tick);
            if playing {
                self.send(frame, &[CONTINUE]);
            }
        } else {
            match (self.running, playing) {
                (false, true) => {
                    if at_start {
                        self.send(frame, &[START]);
                    } else {
                        if let Some(t) = located {
                            position(self, t);
                        }
                        self.send(frame, &[CONTINUE]);
                    }
                }
                (true, false) => self.send(frame, &[STOP]),
                (true, true) => {
                    if let Some(t) = located {
                        if config.restart_on_locate {
                            self.send(frame, &[STOP]);
                            if t <= 0 {
                                self.send(frame, &[START]);
                            } else {
                                position(self, t);
                                self.send(frame, &[CONTINUE]);
                            }
                        } else {
                            position(self, t);
                        }
                    }
                }
                (false, false) => {
                    if let Some(t) = located {
                        position(self, t);
                    }
                }
            }
        }
        self.running = playing;
        config
    }

    /// The song position jumped to `tick` at engine frame `frame` (a loop wrap): the position is
    /// sent, nothing else (a receiver keeps running).
    pub(crate) fn wrapped(&self, frame: u64, tick: i64, config: SyncConfig) {
        if config.enabled && config.song_position {
            self.send(frame, &song_position_bytes(tick));
        }
    }

    /// One MIDI clock at engine frame `frame`.
    pub(crate) fn clock(&self, frame: u64) {
        self.send(frame, &[CLOCK]);
    }

    /// The free-running clock of a stopped transport for the batch at engine frame `frame`, at
    /// `ticks_per_frame`: a clock every [`TICKS_PER_CLOCK`] ticks of time, the first right away.
    pub(crate) fn idle(&mut self, frame: u64, batch: usize, ticks_per_frame: f64) {
        if ticks_per_frame <= 0.0 || !ticks_per_frame.is_finite() {
            return;
        }
        let span = batch as f64 * ticks_per_frame;
        let mut t = self.idle_to_next;
        while t < span {
            let offset = (t / ticks_per_frame).floor() as u64;
            self.clock(frame + offset.min(batch as u64 - 1));
            t += TICKS_PER_CLOCK as f64;
        }
        self.idle_to_next = t - span;
    }
}

/// What a clock source tells the player (see [`ExternalClock`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ExternalState {
    /// The tempo measured from the clocks, in BPM; `None` until there is an estimate.
    pub tempo: Option<f64>,
    /// Whether the sender runs (Start or Continue came, Stop did not).
    pub running: bool,
    /// The position the sender last located to, in ticks (Start: 0; a Song Position Pointer).
    pub position: i64,
    /// How many times the sender located (Start and Song Position Pointer count).
    pub locates: u64,
    /// How many clocks have arrived; a count that stops moving means the clock was lost.
    pub clocks: u64,
}

/// Lock-free (seqlock) cell holding an [`ExternalState`]: one writer (the clock input's thread),
/// any number of readers (the audio thread among them). Also holds the switch that tells the
/// player to follow it.
#[derive(Debug, Default)]
pub struct ExternalClock {
    enabled: AtomicBool,
    seq: AtomicU64,
    tempo: AtomicU64,
    flags: AtomicU64,
    position: AtomicU64,
    locates: AtomicU64,
    clocks: AtomicU64,
}

impl ExternalClock {
    /// An empty cell, not followed.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Whether the player follows the cell.
    pub fn following(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Switches following on or off (taken at the next batch).
    pub fn set_following(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// Publishes `state` (single writer; wait-free: a few stores).
    pub fn publish(&self, state: &ExternalState) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release); // the odd sequence is visible before the data changes
        let (has, bpm) = state.tempo.map_or((0, 0.0), |t| (1, t));
        self.tempo.store(bpm.to_bits(), Ordering::Relaxed);
        self.flags
            .store(u64::from(state.running) | has << 1, Ordering::Relaxed);
        self.position
            .store(state.position as u64, Ordering::Relaxed);
        self.locates.store(state.locates, Ordering::Relaxed);
        self.clocks.store(state.clocks, Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// The latest state (the default before the first publish). Lock-free: retries only while a
    /// publish is in progress.
    pub fn read(&self) -> ExternalState {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before == 0 {
                return ExternalState::default();
            }
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let flags = self.flags.load(Ordering::Relaxed);
            let state = ExternalState {
                tempo: (flags & 2 != 0).then(|| f64::from_bits(self.tempo.load(Ordering::Relaxed))),
                running: flags & 1 != 0,
                position: self.position.load(Ordering::Relaxed) as i64,
                locates: self.locates.load(Ordering::Relaxed),
                clocks: self.clocks.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire); // the data loads happen before the re-check
            if self.seq.load(Ordering::Relaxed) == before {
                return state;
            }
            std::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::{Tempo, TempoSegment};

    const SR: f64 = 48_000.0;

    #[test]
    fn a_constant_tempo_gives_a_clock_every_twenty_fourth_of_a_beat() {
        let seg = [TempoSegment::constant(Tempo::new(120.0).unwrap())];
        let mut frames = Vec::new();
        clocks_in(TempoMap(&seg), SR, 0, 48_000, |f| frames.push(f));
        // One beat is 24_000 frames at 120 BPM: 1000 frames a clock, 48 clocks in a second.
        assert_eq!(frames.len(), 48);
        for (k, f) in frames.iter().enumerate() {
            assert_eq!(*f, k as u64 * 1000);
        }
    }

    #[test]
    fn ranges_that_tile_the_timeline_give_every_clock_exactly_once() {
        let seg = [TempoSegment::constant(Tempo::new(97.3).unwrap())];
        let whole: Vec<u64> = {
            let mut v = Vec::new();
            clocks_in(TempoMap(&seg), SR, 0, 200_000, |f| v.push(f));
            v
        };
        let mut tiled = Vec::new();
        let mut a = 0;
        while a < 200_000 {
            clocks_in(TempoMap(&seg), SR, a, a + 64, |f| tiled.push(f));
            a += 64;
        }
        assert_eq!(whole, tiled);
        assert!(whole.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn the_position_message_counts_sixteenth_notes() {
        assert_eq!(song_position_bytes(0), [0xF2, 0, 0]);
        assert_eq!(song_position_bytes(PPQ), [0xF2, 4, 0]);
        assert_eq!(song_position_bytes(4 * PPQ * 8), [0xF2, 0, 1]);
        assert_eq!(song_position_bytes(-5), [0xF2, 0, 0]);
        assert_eq!(song_position_bytes(i64::MAX), [0xF2, 0x7F, 0x7F]);
        assert_eq!(song_position_ticks(4, 0), PPQ);
        assert_eq!(song_position_ticks(0, 1), 128 * PPQ / 4);
    }

    #[test]
    fn the_external_cell_returns_what_was_published() {
        let cell = ExternalClock::new();
        assert_eq!(cell.read(), ExternalState::default());
        let s = ExternalState {
            tempo: Some(133.25),
            running: true,
            position: -5,
            locates: 3,
            clocks: 99,
        };
        cell.publish(&s);
        assert_eq!(cell.read(), s);
        cell.publish(&ExternalState::default());
        assert_eq!(cell.read(), ExternalState::default());
    }
}
