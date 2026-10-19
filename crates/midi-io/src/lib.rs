//! Atomik Rack MIDI input: port listing, a pure parser
//! and jitter-free mapping of event times to engine frames.
//!
//! - [`parse`] turns raw MIDI bytes into [`NoteKind`]s: note on/off (velocity 0 is
//!   off), CC 120/123 → `AllOff`, the other CCs, pitch bend and both pressures,
//!   running status, and realtime bytes (`0xF8..=0xFF`) ignored anywhere without breaking
//!   running status. It never panics.
//! - [`event_frame`] is the pure jitter-free mapping from an event instant to an
//!   engine frame, using the `(frame, instant)` pair the audio callback publishes through
//!   [`FrameClock`].
//! - [`open_input`] connects a midir input port. Its callback runs on midir's thread (not the
//!   audio thread): it reads the wall clock, maps the event to a frame and pushes a
//!   [`ScheduledNote`] through the [`LiveNoteSender`]. Before the audio callback has published
//!   the clock, notes are delivered at frame 0 (offset 0 of the next batch). Frames pushed by
//!   one input never decrease.

use atomik_core::NoteKind;
use atomik_device_sdk::SocketId;
use atomik_engine::{DeviceInstanceId, FrameClock, LiveNoteSender, ScheduledNote};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Client name registered with the MIDI backend.
const CLIENT_NAME: &str = "atomik-rack";

/// MIDI errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MidiError {
    /// The MIDI backend failed to initialise.
    #[error("MIDI backend error: {0}")]
    Backend(String),
    /// No input port matched the requested name, or the match was ambiguous.
    #[error("MIDI input not found or ambiguous: {0}")]
    PortNotFound(String),
}

/// Running-status parser state (one per input port).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunningStatus {
    /// Last channel-voice status byte (`0x80..=0xEF`), or 0 when there is none.
    status: u8,
}

/// Parses one MIDI message (possibly relying on running status). Returns the note action
/// for note on/off (velocity 0 = off), CC 120/123 (→ `AllOff`), and the
/// channel messages: pitch bend (14 bit, centre 8192 → 0, -1..1), every other CC
/// ([`NoteKind::Cc`], 0..1), channel pressure and polyphonic key pressure; `None` for anything
/// else (program change, system messages) or malformed bytes. The MIDI channel is dropped: all
/// channels reach the focused or armed instrument. Never panics.
pub fn parse(bytes: &[u8], state: &mut RunningStatus) -> Option<NoteKind> {
    let mut status = 0u8;
    let mut data = [0u8; 2];
    let mut len = 0usize;
    // Realtime bytes may appear anywhere and never affect running status.
    for b in bytes.iter().copied().filter(|b| *b < 0xF8) {
        if b >= 0x80 {
            if status != 0 || len != 0 {
                return None; // a status byte inside a message: malformed
            }
            if b >= 0xF0 {
                // System common / SysEx: cancels running status; not a note.
                state.status = 0;
                return None;
            }
            state.status = b;
            status = b;
        } else {
            if status == 0 {
                status = state.status;
                if status == 0 {
                    return None; // data without any status
                }
            }
            if len < data.len() {
                data[len] = b;
                len += 1;
            }
        }
    }
    // Program change and channel pressure carry one data byte, everything else two.
    let need = if matches!(status & 0xF0, 0xC0 | 0xD0) {
        1
    } else {
        2
    };
    if len < need {
        return None;
    }
    let [d0, d1] = data;
    match status & 0xF0 {
        0xA0 => Some(NoteKind::PolyPressure {
            key: d0,
            value: f32::from(d1) / 127.0,
        }),
        0xB0 if d0 != 120 && d0 != 123 => Some(NoteKind::Cc {
            controller: d0,
            value: f32::from(d1) / 127.0,
        }),
        0xD0 => Some(NoteKind::ChannelPressure {
            value: f32::from(d0) / 127.0,
        }),
        0xE0 => {
            let bend = i32::from(d0) | (i32::from(d1) << 7);
            Some(NoteKind::PitchBend {
                value: (bend - 8192) as f32 / 8192.0,
            })
        }
        0x80 => Some(NoteKind::Off { key: d0 }),
        0x90 if d1 == 0 => Some(NoteKind::Off { key: d0 }),
        0x90 => Some(NoteKind::On {
            key: d0,
            velocity: f32::from(d1) / 127.0,
        }),
        0xB0 if d0 == 120 || d0 == 123 => Some(NoteKind::AllOff),
        _ => None,
    }
}

/// Maps an event received at `at` to an engine frame (jitter-free):
/// `frame = clock_frame + (at − clock_at)·sample_rate + latency_frames`, saturating at 0.
pub fn event_frame(
    clock_frame: u64,
    clock_at: Instant,
    at: Instant,
    sample_rate: f64,
    latency_frames: u64,
) -> u64 {
    let (later, elapsed) = match at.checked_duration_since(clock_at) {
        Some(d) => (true, d),
        None => (false, clock_at.saturating_duration_since(at)),
    };
    let frames = (elapsed.as_secs_f64() * sample_rate).round();
    // `as` saturates for out-of-range floats; NaN and negatives map to 0.
    let frames = if frames > 0.0 { frames as u64 } else { 0 };
    let base = clock_frame.saturating_add(latency_frames);
    if later {
        base.saturating_add(frames)
    } else {
        base.saturating_sub(frames)
    }
}

/// Lists MIDI input port names.
pub fn list_inputs() -> Result<Vec<String>, MidiError> {
    let input = new_backend()?;
    port_names(&input)
}

/// An open MIDI input; dropping it closes the port.
pub struct MidiInput {
    _connection: midir::MidiInputConnection<Forwarder>,
    port: String,
    dropped: Arc<AtomicU64>,
}

impl std::fmt::Debug for MidiInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiInput")
            .field("port", &self.port)
            .field("dropped", &self.dropped())
            .finish_non_exhaustive()
    }
}

impl MidiInput {
    /// The full name of the connected port.
    pub fn port_name(&self) -> &str {
        &self.port
    }

    /// Notes that could not be queued because the live note queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Opens input port `name` (exact or unique case-insensitive substring) and forwards parsed
/// notes to `target` through `sender`, timestamped with `clock` (jitter-free, latency
/// `latency_frames` = one host buffer).
pub fn open_input(
    name: &str,
    target: (DeviceInstanceId, SocketId),
    sender: LiveNoteSender,
    clock: Arc<FrameClock>,
    sample_rate: f64,
    latency_frames: u64,
) -> Result<MidiInput, MidiError> {
    let mut input = new_backend()?;
    // SysEx, MIDI clock and active sensing are out of scope; drop them at the source.
    input.ignore(midir::Ignore::All);
    let names = port_names(&input)?;
    let index = match_port(&names, name)?;
    let ports = input.ports();
    let port = ports
        .get(index)
        .ok_or_else(|| MidiError::PortNotFound(name.to_string()))?;
    let dropped = Arc::new(AtomicU64::new(0));
    let forwarder = Forwarder {
        state: RunningStatus::default(),
        timer: NoteTimer::new(sample_rate, latency_frames),
        sender,
        clock,
        target,
        dropped: Arc::clone(&dropped),
    };
    let connection = input
        .connect(
            port,
            "atomik-rack-input",
            |_stamp, bytes, fwd: &mut Forwarder| fwd.on_message(bytes),
            forwarder,
        )
        .map_err(|e| MidiError::Backend(e.to_string()))?;
    Ok(MidiInput {
        _connection: connection,
        port: names[index].clone(),
        dropped,
    })
}

fn new_backend() -> Result<midir::MidiInput, MidiError> {
    midir::MidiInput::new(CLIENT_NAME).map_err(|e| MidiError::Backend(e.to_string()))
}

fn port_names(input: &midir::MidiInput) -> Result<Vec<String>, MidiError> {
    input
        .ports()
        .iter()
        .map(|p| {
            input
                .port_name(p)
                .map_err(|e| MidiError::Backend(e.to_string()))
        })
        .collect()
}

/// State owned by the midir callback (midir's thread, not the audio thread).
struct Forwarder {
    state: RunningStatus,
    timer: NoteTimer,
    sender: LiveNoteSender,
    clock: Arc<FrameClock>,
    target: (DeviceInstanceId, SocketId),
    dropped: Arc<AtomicU64>,
}

impl Forwarder {
    fn on_message(&mut self, bytes: &[u8]) {
        let at = timing::now();
        let Some(kind) = parse(bytes, &mut self.state) else {
            return;
        };
        let frame = self.timer.frame(self.clock.read(), at);
        let note = ScheduledNote {
            frame,
            device: self.target.0,
            socket: self.target.1,
            kind,
        };
        if self.sender.push(note).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Timestamps live notes for one input: the jitter-free mapping, frame 0 before the first
/// clock publish, and never decreasing (the live queue expects non-decreasing frames).
#[derive(Debug, Clone, Copy)]
struct NoteTimer {
    sample_rate: f64,
    latency_frames: u64,
    last: u64,
}

impl NoteTimer {
    fn new(sample_rate: f64, latency_frames: u64) -> Self {
        Self {
            sample_rate,
            latency_frames,
            last: 0,
        }
    }

    fn frame(&mut self, clock: Option<(u64, Instant)>, at: Instant) -> u64 {
        let mapped = clock.map_or(0, |(frame, clock_at)| {
            event_frame(frame, clock_at, at, self.sample_rate, self.latency_frames)
        });
        self.last = self.last.max(mapped);
        self.last
    }
}

/// Exact name first, then a unique case-insensitive substring.
fn match_port(names: &[String], query: &str) -> Result<usize, MidiError> {
    if let Some(i) = names.iter().position(|n| n == query) {
        return Ok(i);
    }
    let needle = query.to_lowercase();
    let hits: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| !needle.is_empty() && n.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => Err(MidiError::PortNotFound(format!(
            "no input matches \"{query}\" (available: {names:?})"
        ))),
        many => Err(MidiError::PortNotFound(format!(
            "\"{query}\" is ambiguous: {:?}",
            many.iter().map(|&i| &names[i]).collect::<Vec<_>>()
        ))),
    }
}

mod timing {
    use std::time::Instant;

    /// Wall-clock read for live MIDI event timing only.
    #[allow(clippy::disallowed_methods)] // Justification: live input timing. The instant
    // only places a live, inherently non-deterministic event on the timeline; it never feeds DSP,
    // so rendering stays deterministic for a given note sequence.
    pub(super) fn now() -> Instant {
        Instant::now()
    }
}

#[cfg(test)]
atomik_testkit::install_alloc_guard!();

#[cfg(test)]
mod tests;
