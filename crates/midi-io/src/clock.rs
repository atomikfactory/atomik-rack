//! MIDI clock in: the realtime messages of a sender (Clock, Start, Continue, Stop and Song
//! Position Pointer) turned into the state the song player follows
//! ([`atomik_engine::ExternalClock`]).
//!
//! * [`ClockFollower`] is the pure part: bytes and the instant they arrived in, the state out. It
//!   measures the tempo from the spacing of the clocks (the mean of the last [`WINDOW`] intervals,
//!   one beat, so jitter of the port averages out while a tempo change still shows within a
//!   beat) and counts the clocks so the player can tell when they stop coming.
//! * [`open_clock_in`] connects a midir input port and feeds a follower from its callback (midir's
//!   thread, not the audio thread). The callback does a few atomic stores per message.
//!
//! The follower tracks no phase: the player takes the tempo and the transport from it and runs on
//! its own frame counter (see the engine's player documentation).

use crate::{MidiError, match_port, new_backend, port_names, timing};
use atomik_engine::{ExternalClock, ExternalState, SONG_POSITION, song_position_ticks};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Clock intervals the tempo is the mean of: one quarter note.
pub const WINDOW: usize = 24;

/// Intervals needed before there is a tempo (a quarter of a beat).
const MIN_INTERVALS: usize = 6;

/// A gap this long between two clocks (a quarter note at 20 BPM is 3 s, a clock 125 ms) means the
/// clock was lost and started again: the measurement begins afresh.
const RESTART_AFTER: Duration = Duration::from_millis(500);

/// The measured tempo of one sender and the state of its transport.
#[derive(Debug)]
pub struct ClockFollower {
    cell: Arc<ExternalClock>,
    state: ExternalState,
    /// Arrival times of the last clocks, oldest first once the ring is full.
    stamps: [Option<Instant>; WINDOW + 1],
    next: usize,
    filled: usize,
}

impl ClockFollower {
    /// A follower that publishes to `cell`.
    pub fn new(cell: Arc<ExternalClock>) -> Self {
        Self {
            cell,
            state: ExternalState::default(),
            stamps: [None; WINDOW + 1],
            next: 0,
            filled: 0,
        }
    }

    /// The state published last.
    pub fn state(&self) -> ExternalState {
        self.state
    }

    /// Takes one message that arrived at `at`: a real-time byte or a Song Position Pointer
    /// (anything else is ignored). Publishes when the state changed.
    pub fn on_message(&mut self, bytes: &[u8], at: Instant) {
        match bytes {
            [0xF8, ..] => self.clock(at),
            // Start: from the beginning.
            [0xFA, ..] => {
                self.state.running = true;
                self.state.position = 0;
                self.state.locates += 1;
            }
            [0xFB, ..] => self.state.running = true,
            [0xFC, ..] => self.state.running = false,
            [SONG_POSITION, lsb, msb, ..] => {
                self.state.position = song_position_ticks(*lsb, *msb);
                self.state.locates += 1;
            }
            _ => return,
        }
        self.cell.publish(&self.state);
    }

    fn clock(&mut self, at: Instant) {
        self.state.clocks += 1;
        let last = self.latest();
        if last.is_some_and(|l| at.saturating_duration_since(l) > RESTART_AFTER) {
            self.filled = 0;
        }
        self.stamps[self.next] = Some(at);
        self.next = (self.next + 1) % self.stamps.len();
        self.filled = (self.filled + 1).min(self.stamps.len());
        if self.filled > MIN_INTERVALS
            && let (Some(first), Some(last)) = (self.oldest(), self.latest())
        {
            let intervals = (self.filled - 1) as f64;
            let secs = last.saturating_duration_since(first).as_secs_f64() / intervals;
            if secs > 0.0 {
                let bpm = 60.0 / (secs * 24.0);
                if (20.0..=999.0).contains(&bpm) {
                    self.state.tempo = Some(bpm);
                }
            }
        }
    }

    fn latest(&self) -> Option<Instant> {
        if self.filled == 0 {
            return None;
        }
        let n = self.stamps.len();
        self.stamps[(self.next + n - 1) % n]
    }

    fn oldest(&self) -> Option<Instant> {
        if self.filled == 0 {
            return None;
        }
        let n = self.stamps.len();
        self.stamps[(self.next + n - self.filled) % n]
    }
}

/// An open MIDI clock input; dropping it closes the port.
pub struct ClockInput {
    _connection: midir::MidiInputConnection<ClockFollower>,
    port: String,
}

impl std::fmt::Debug for ClockInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClockInput")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl ClockInput {
    /// The full name of the connected port.
    pub fn port_name(&self) -> &str {
        &self.port
    }
}

/// Opens input port `name` (exact or unique case-insensitive substring) and feeds `cell` with
/// what its clock says. System exclusive and active sensing are dropped at the source; the
/// real-time messages are kept.
///
/// # Errors
/// [`MidiError::PortNotFound`] when no port matches, [`MidiError::Backend`] otherwise.
pub fn open_clock_in(name: &str, cell: Arc<ExternalClock>) -> Result<ClockInput, MidiError> {
    let mut input = new_backend()?;
    input.ignore(midir::Ignore::SysexAndActiveSense);
    let names = port_names(&input)?;
    let index = match_port(&names, name)?;
    let ports = input.ports();
    let port = ports
        .get(index)
        .ok_or_else(|| MidiError::PortNotFound(name.to_string()))?;
    let connection = input
        .connect(
            port,
            "atomik-rack-clock-in",
            |_stamp, bytes, follower: &mut ClockFollower| {
                follower.on_message(bytes, timing::now());
            },
            ClockFollower::new(cell),
        )
        .map_err(|e| MidiError::Backend(e.to_string()))?;
    Ok(ClockInput {
        _connection: connection,
        port: names[index].clone(),
    })
}

#[cfg(test)]
mod tests;
