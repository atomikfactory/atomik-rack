//! Live MIDI input with a retargetable destination, from any number of ports at once.
//!
//! `atomik_midi_io::open_input` binds one fixed `(device, socket)` target and consumes the
//! engine's single [`LiveNoteSender`], so it cannot follow the selected device. This module is
//! the app-side workaround (contract gap reported): it reuses midi-io's pure [`parse`] and
//! [`event_frame`] and connects midir itself. Each note is routed through a [`NoteRouter`] to
//! the target currently published in the shared [`NoteTarget`] cell, so one sender serves
//! every device.
//!
//! # Several ports
//!
//! The engine has one live queue with one producer, so every open port (and the audition and
//! Piano Keys paths) pushes through one [`LiveFeed`] behind a short mutex: the MIDI threads and
//! the UI thread take it, the audio thread never does (it only consumes the queue). Each port
//! has its own parser, running status and [`NoteRouter`], so a note-off follows its note-on
//! whatever the target did in between. The feed keeps the queue ordered across ports (a note is
//! never stamped before one already queued) and never stamps a note more than two buffers past
//! the host frame, so a stamp from a stale clock cannot hold back the notes behind it (the
//! engine only looks at the head of the queue).

use crate::midi_control::ControlHub;
use crate::record::RecordTap;
use crate::target::{NoteRouter, NoteTarget, Target};
use atomik_core::NoteKind;
use atomik_engine::{EngineError, FrameClock, LiveNoteSender, ScheduledNote};
use atomik_midi_io::{MidiError, RunningStatus, event_frame, parse, parse_message};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

/// How many buffers past the host frame a live note may be stamped.
const MAX_AHEAD_BUFFERS: u64 = 2;

/// The engine's live note producer, shared by every MIDI input and the audition path: it orders
/// and bounds the frames of what it queues.
#[derive(Debug)]
pub struct LiveFeed {
    sender: LiveNoteSender,
    /// The frame of the last note queued: the next one is not stamped before it.
    last_frame: u64,
}

impl LiveFeed {
    /// A feed over the engine's live producer.
    pub fn new(sender: LiveNoteSender) -> Self {
        Self {
            sender,
            last_frame: 0,
        }
    }

    /// The frame a note stamped `frame` is queued at: never before the last queued note, never
    /// after `ceiling` (when there is one; the ceiling wins, a note is never held back).
    pub fn admit(&mut self, frame: u64, ceiling: Option<u64>) -> u64 {
        let frame = frame.max(self.last_frame);
        let frame = ceiling.map_or(frame, |c| frame.min(c));
        self.last_frame = frame;
        frame
    }

    /// Queues `note` at its admitted frame.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`] when the live queue is full.
    pub fn push(
        &mut self,
        mut note: ScheduledNote,
        ceiling: Option<u64>,
    ) -> Result<(), EngineError> {
        note.frame = self.admit(note.frame, ceiling);
        self.sender.push(note)
    }
}

/// Everything a MIDI input needs to feed the engine. Cloning it shares the feed and the cells:
/// each open port holds a clone.
#[derive(Debug, Clone)]
pub struct MidiLink {
    /// The engine's live note producer, shared by every port.
    pub feed: Arc<Mutex<LiveFeed>>,
    /// The audio callback's frame clock.
    pub clock: Arc<FrameClock>,
    /// The current notes target.
    pub target: Arc<NoteTarget>,
    /// Engine sample rate in Hz.
    pub sample_rate: f64,
    /// Scheduling latency in frames (one host buffer); updated when the host reports it.
    pub latency: Arc<AtomicU64>,
    /// Recording capture: every note sent is offered to it.
    pub tap: Arc<RecordTap>,
    /// The control path: every parsed message goes to its inbox for the mappings, and its
    /// consume list decides which messages never reach the notes.
    pub hub: Arc<ControlHub>,
    /// The port's notes play the target (a keyboard). Off: the port feeds the mappings only.
    pub plays_notes: bool,
}

impl MidiLink {
    /// A link over the engine's live producer.
    pub fn new(
        sender: LiveNoteSender,
        clock: Arc<FrameClock>,
        target: Arc<NoteTarget>,
        sample_rate: f64,
        latency: Arc<AtomicU64>,
        tap: Arc<RecordTap>,
        hub: Arc<ControlHub>,
    ) -> Self {
        Self {
            feed: Arc::new(Mutex::new(LiveFeed::new(sender))),
            clock,
            target,
            sample_rate,
            latency,
            tap,
            hub,
            plays_notes: true,
        }
    }

    /// This link for a port that plays notes or only feeds the mappings.
    pub fn with_plays_notes(&self, plays_notes: bool) -> Self {
        Self {
            plays_notes,
            ..self.clone()
        }
    }

    /// Queues `note` through the shared feed, stamped no later than two buffers past the host
    /// frame now.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`] when the live queue is full.
    pub fn push(&self, note: ScheduledNote) -> Result<(), EngineError> {
        let ceiling = self.ceiling(timing::now());
        let mut feed = self.feed.lock().unwrap_or_else(PoisonError::into_inner);
        feed.push(note, ceiling)
    }

    /// The latest frame a note may be stamped for at `now`: the host frame plus two buffers.
    /// `None` before the clock published.
    fn ceiling(&self, now: Instant) -> Option<u64> {
        let (frame, at) = self.clock.read()?;
        let latency = self.latency.load(Ordering::Relaxed);
        let elapsed = now.saturating_duration_since(at).as_secs_f64() * self.sample_rate;
        Some(
            frame
                .saturating_add(elapsed as u64)
                .saturating_add(latency.saturating_mul(MAX_AHEAD_BUFFERS)),
        )
    }

    /// Sends one note to `target` as if it arrived now (audition): timestamped like
    /// live MIDI (frame clock plus one buffer of latency), on the same live queue.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`] when the live queue is full.
    pub fn send_now(&self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        let note = self.note_now(target, kind);
        self.push(note)
    }

    /// [`Self::send_now`] for played input (the Piano Keys): the note is also offered to the
    /// recording tap, as a note from the MIDI input is, so a take captures it.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`] when the live queue is full.
    pub fn play_now(&self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        let note = self.note_now(target, kind);
        self.push(note)?;
        self.tap.offer(&note);
        Ok(())
    }

    fn note_now(&self, target: Target, kind: NoteKind) -> ScheduledNote {
        let latency = self.latency.load(Ordering::Relaxed);
        let frame = self.clock.read().map_or(0, |(frame, at)| {
            event_frame(frame, at, timing::now(), self.sample_rate, latency)
        });
        ScheduledNote {
            frame,
            device: target.0,
            socket: target.1,
            kind,
        }
    }
}

/// Turns raw MIDI messages into scheduled notes for the current target (no I/O; runs on the
/// MIDI thread and never allocates).
#[derive(Debug, Clone)]
pub struct NoteForwarder {
    state: RunningStatus,
    router: NoteRouter,
    last_frame: u64,
}

impl Default for NoteForwarder {
    fn default() -> Self {
        Self::new()
    }
}

impl NoteForwarder {
    /// A forwarder with no running status and no held notes.
    pub fn new() -> Self {
        Self {
            state: RunningStatus::default(),
            router: NoteRouter::new(),
            last_frame: 0,
        }
    }

    /// Handles one message received at `at`: parses it, timestamps it jitter-free from
    /// `clock` (frame 0 before the first publish; never decreasing), routes it and calls
    /// `emit` for each note to queue.
    #[allow(clippy::too_many_arguments)]
    pub fn forward(
        &mut self,
        bytes: &[u8],
        at: Instant,
        clock: Option<(u64, Instant)>,
        sample_rate: f64,
        latency_frames: u64,
        target: &NoteTarget,
        emit: impl FnMut(ScheduledNote),
    ) {
        self.forward_with(
            bytes,
            at,
            clock,
            sample_rate,
            latency_frames,
            target,
            false,
            emit,
        );
    }

    /// [`Self::forward`] for a message a mapping may have consumed: a consumed message is
    /// parsed (so running status stays right) and timestamped but not routed, so it plays no
    /// note.
    #[allow(clippy::too_many_arguments)]
    pub fn forward_with(
        &mut self,
        bytes: &[u8],
        at: Instant,
        clock: Option<(u64, Instant)>,
        sample_rate: f64,
        latency_frames: u64,
        target: &NoteTarget,
        consumed: bool,
        mut emit: impl FnMut(ScheduledNote),
    ) {
        let Some(kind) = parse(bytes, &mut self.state) else {
            return;
        };
        let mapped = clock.map_or(0, |(frame, clock_at)| {
            event_frame(frame, clock_at, at, sample_rate, latency_frames)
        });
        // The live queue expects non-decreasing frames.
        self.last_frame = self.last_frame.max(mapped);
        let frame = self.last_frame;
        if consumed {
            return;
        }
        self.router
            .route(kind, target.get(), |(device, socket), kind| {
                emit(ScheduledNote {
                    frame,
                    device,
                    socket,
                    kind,
                });
            });
    }
}

/// What one MIDI input does with a message: the state of one port, owned by its callback. A port
/// has its own parsers (running status) and its own [`NoteRouter`].
pub struct PortInput {
    forwarder: NoteForwarder,
    /// The parser of the control path (its own running status).
    control_state: RunningStatus,
    /// The port's name, shared with every message sent to the hub.
    port: Arc<str>,
    link: MidiLink,
    dropped: Arc<AtomicU64>,
}

impl PortInput {
    /// The input of port `port` feeding the engine through `link`.
    pub fn new(port: &str, link: MidiLink) -> Self {
        Self {
            forwarder: NoteForwarder::new(),
            control_state: RunningStatus::default(),
            port: Arc::from(port),
            link,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Notes dropped because the live queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Handles one message from the port: the mappings see it, a message a mapping consumes (or
    /// any message while learning, or any message of a mapping-only port) plays no note, the
    /// rest is routed to the target and queued.
    pub fn on_message(&mut self, bytes: &[u8]) {
        let at = timing::now();
        let link = &self.link;
        let dropped = &self.dropped;
        let latency = link.latency.load(Ordering::Relaxed);
        let clock = link.clock.read();
        // The control path first: the mappings see every message, and a message a mapping
        // consumes (or any message while learning) plays no note.
        let consumed = parse_message(bytes, &mut self.control_state).is_some_and(|m| {
            link.hub.push(&self.port, m);
            link.hub.consumes(&self.port, &m)
        }) || !link.plays_notes;
        self.forwarder.forward_with(
            bytes,
            at,
            clock,
            link.sample_rate,
            latency,
            &link.target,
            consumed,
            |note| {
                if link.push(note).is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
                // Recording: kept only for armed devices; never allocates.
                link.tap.offer(&note);
            },
        );
    }
}

/// An open MIDI input port feeding the engine.
pub struct MidiConnection {
    connection: midir::MidiInputConnection<PortInput>,
    port: String,
    dropped: Arc<AtomicU64>,
}

impl std::fmt::Debug for MidiConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiConnection")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl MidiConnection {
    /// Opens input port `name` (exact name or unique case-insensitive substring), feeding the
    /// engine through `link` (shared with every other open port).
    pub fn open(name: &str, link: MidiLink) -> Result<Self, MidiError> {
        let mut input =
            midir::MidiInput::new("atomik-rack").map_err(|e| MidiError::Backend(e.to_string()))?;
        // SysEx, clock and active sensing are out of scope.
        input.ignore(midir::Ignore::All);
        let ports = input.ports();
        let names: Vec<String> = ports
            .iter()
            .map(|p| input.port_name(p).unwrap_or_default())
            .collect();
        let index = match_port(&names, name)?;
        let state = PortInput::new(&names[index], link);
        let dropped = Arc::clone(&state.dropped);
        let connection = input
            .connect(
                &ports[index],
                "atomik-rack-input",
                |_stamp, bytes, state: &mut PortInput| state.on_message(bytes),
                state,
            )
            .map_err(|e| MidiError::Backend(e.to_string()))?;
        Ok(Self {
            connection,
            port: names[index].clone(),
            dropped,
        })
    }

    /// The connected port's full name.
    pub fn port_name(&self) -> &str {
        &self.port
    }

    /// Notes dropped because the live queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Closes the port.
    pub fn close(self) {
        drop(self.connection.close());
    }
}

/// Exact name first, then a unique case-insensitive substring (as `atomik-midi-io`).
pub fn match_port(names: &[String], query: &str) -> Result<usize, MidiError> {
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
    #[allow(clippy::disallowed_methods)] // Justification: live input timing, as in
    // atomik-midi-io. The instant only places a live, inherently non-deterministic event on
    // the timeline; it never feeds DSP.
    pub(super) fn now() -> Instant {
        Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::NoteKind;
    use atomik_device_sdk::SocketId;
    use atomik_engine::DeviceInstanceId;
    use std::time::Duration;

    fn run(
        f: &mut NoteForwarder,
        bytes: &[u8],
        at: Instant,
        clock: Option<(u64, Instant)>,
        target: &NoteTarget,
    ) -> Vec<ScheduledNote> {
        let mut out = Vec::new();
        f.forward(bytes, at, clock, 48_000.0, 256, target, |n| out.push(n));
        out
    }

    fn target(id: u32) -> NoteTarget {
        let t = NoteTarget::new();
        t.set(Some((DeviceInstanceId(id), SocketId(9))));
        t
    }

    #[test]
    fn a_note_on_is_scheduled_one_latency_after_the_clock() {
        let base = timing::now();
        let mut f = NoteForwarder::new();
        let at = base + Duration::from_millis(10);
        let out = run(&mut f, &[0x90, 60, 127], at, Some((1000, base)), &target(3));
        assert_eq!(
            out,
            vec![ScheduledNote {
                frame: 1000 + 480 + 256,
                device: DeviceInstanceId(3),
                socket: SocketId(9),
                kind: NoteKind::On {
                    key: 60,
                    velocity: 1.0,
                },
            }]
        );
    }

    #[test]
    fn before_the_first_clock_publish_notes_land_at_frame_0() {
        let mut f = NoteForwarder::new();
        let out = run(&mut f, &[0x90, 60, 64], timing::now(), None, &target(1));
        assert_eq!(out[0].frame, 0);
    }

    #[test]
    fn frames_never_decrease() {
        let base = timing::now();
        let mut f = NoteForwarder::new();
        let t = target(1);
        let first = run(&mut f, &[0x90, 60, 64], base, Some((10_000, base)), &t);
        let earlier = run(&mut f, &[0x80, 60, 0], base, Some((5_000, base)), &t);
        assert!(earlier[0].frame >= first[0].frame);
    }

    #[test]
    fn non_note_messages_and_missing_targets_emit_nothing() {
        let mut f = NoteForwarder::new();
        let now = timing::now();
        // Program change and system messages are not notes, and nothing goes nowhere.
        assert!(run(&mut f, &[0xC0, 5], now, None, &target(1)).is_empty());
        assert!(run(&mut f, &[0xF0, 0x7E, 0xF7], now, None, &target(1)).is_empty());
        assert!(run(&mut f, &[0x90, 60, 64], now, None, &NoteTarget::new()).is_empty());
        assert!(run(&mut f, &[0xE0, 0, 64], now, None, &NoteTarget::new()).is_empty());
        assert!(run(&mut f, &[0xB0, 1, 64], now, None, &NoteTarget::new()).is_empty());
    }

    /// Pitch bend, mod wheel, sustain and the pressures reach the focused or
    /// armed instrument; a key's pressure follows the device that holds the key.
    #[test]
    fn channel_messages_follow_the_current_target_and_key_pressure_follows_the_held_key() {
        let mut f = NoteForwarder::new();
        let now = timing::now();
        let t = target(1);
        let kinds = |out: Vec<ScheduledNote>| -> Vec<(u32, NoteKind)> {
            out.iter().map(|n| (n.device.0, n.kind)).collect()
        };
        assert_eq!(
            kinds(run(&mut f, &[0xE0, 0, 96], now, None, &t)),
            [(1, NoteKind::PitchBend { value: 0.5 })]
        );
        assert_eq!(
            kinds(run(&mut f, &[0xB0, 1, 127], now, None, &t)),
            [(
                1,
                NoteKind::Cc {
                    controller: 1,
                    value: 1.0
                }
            )]
        );
        assert_eq!(
            kinds(run(&mut f, &[0xB0, 64, 127], now, None, &t)),
            [(
                1,
                NoteKind::Cc {
                    controller: 64,
                    value: 1.0
                }
            )]
        );
        assert_eq!(
            kinds(run(&mut f, &[0xD0, 127], now, None, &t)),
            [(1, NoteKind::ChannelPressure { value: 1.0 })]
        );
        // A note is held on device 1; the selection moves to device 2.
        run(&mut f, &[0x90, 60, 64], now, None, &t);
        t.set(Some((DeviceInstanceId(2), SocketId(9))));
        assert_eq!(
            kinds(run(&mut f, &[0xE0, 0, 64], now, None, &t)),
            [(2, NoteKind::PitchBend { value: 0.0 })],
            "channel messages go to the new target"
        );
        assert_eq!(
            kinds(run(&mut f, &[0xA0, 60, 127], now, None, &t)),
            [(
                1,
                NoteKind::PolyPressure {
                    key: 60,
                    value: 1.0
                }
            )],
            "key pressure goes where the note started"
        );
        assert_eq!(
            kinds(run(&mut f, &[0xA0, 61, 127], now, None, &t)),
            [(
                2,
                NoteKind::PolyPressure {
                    key: 61,
                    value: 1.0
                }
            )],
            "a key nobody holds goes to the current target"
        );
    }

    #[test]
    fn a_retarget_moves_new_notes_but_releases_held_ones_where_they_started() {
        let mut f = NoteForwarder::new();
        let now = timing::now();
        let t = target(1);
        run(&mut f, &[0x90, 60, 64], now, None, &t);
        t.set(Some((DeviceInstanceId(2), SocketId(9))));
        let on = run(&mut f, &[0x90, 62, 64], now, None, &t);
        assert_eq!(on[0].device, DeviceInstanceId(2));
        let off = run(&mut f, &[0x80, 60, 0], now, None, &t);
        assert_eq!(off[0].device, DeviceInstanceId(1));
    }

    #[test]
    fn running_status_is_kept_across_messages() {
        let mut f = NoteForwarder::new();
        let now = timing::now();
        let t = target(1);
        run(&mut f, &[0x90, 60, 64], now, None, &t);
        let out = run(&mut f, &[62, 64], now, None, &t);
        assert!(matches!(out[0].kind, NoteKind::On { key: 62, .. }));
    }

    #[test]
    fn ports_match_exactly_then_by_unique_substring() {
        let names = vec![
            "Example Keyboard".to_string(),
            "Example Keyboard MIDI 2".to_string(),
        ];
        assert_eq!(match_port(&names, "Example Keyboard").unwrap(), 0);
        assert_eq!(match_port(&names, "midi 2").unwrap(), 1);
        assert!(matches!(
            match_port(&names, "keyboard"),
            Err(MidiError::PortNotFound(_))
        ));
        assert!(matches!(
            match_port(&names, "x"),
            Err(MidiError::PortNotFound(_))
        ));
        assert!(matches!(
            match_port(&names, ""),
            Err(MidiError::PortNotFound(_))
        ));
    }

    fn link_with_tap() -> (MidiLink, Arc<RecordTap>, Arc<NoteTarget>) {
        let cfg = atomik_engine::EngineConfig {
            sample_rate: atomik_core::SampleRate::new(48_000.0).unwrap(),
            tempo: atomik_core::Tempo::new(120.0).unwrap(),
            input_channels: 0,
            output_channels: 2,
            seed: 0,
        };
        let (_engine, mut handle) = atomik_engine::new_engine(cfg);
        let tap = Arc::new(RecordTap::new());
        let target = Arc::new(target(3));
        let link = MidiLink::new(
            handle.live_notes().unwrap(),
            Arc::new(FrameClock::new()),
            Arc::clone(&target),
            48_000.0,
            Arc::new(AtomicU64::new(256)),
            Arc::clone(&tap),
            Arc::new(ControlHub::new()),
        );
        (link, tap, target)
    }

    #[test]
    fn notes_to_armed_devices_are_offered_to_the_recording_tap() {
        let (link, tap, target) = link_with_tap();
        let mut fwd = PortInput::new("Example", link.clone());
        tap.set_armed(&[DeviceInstanceId(3)]);
        fwd.on_message(&[0x90, 60, 100]);
        fwd.on_message(&[0x80, 60, 0]);
        target.set(Some((DeviceInstanceId(4), SocketId(9))));
        fwd.on_message(&[0x90, 62, 100]);
        let mut out = Vec::new();
        tap.drain(&mut out);
        assert_eq!(out.len(), 2, "only the armed device: {out:?}");
        assert!(matches!(out[0].kind, NoteKind::On { key: 60, .. }));
        assert!(matches!(out[1].kind, NoteKind::Off { key: 60 }));
        assert_eq!(out[0].device, DeviceInstanceId(3));
        // Audition goes through the same live queue.
        assert!(
            link.send_now((DeviceInstanceId(3), SocketId(9)), NoteKind::Off { key: 1 })
                .is_ok()
        );
    }

    /// Played input (the Piano Keys) is offered to the recording tap as a MIDI note is: the same
    /// armed-device filter, the same events.
    #[test]
    fn played_notes_reach_the_recording_tap_like_midi_notes() {
        let (link, tap, _target) = link_with_tap();
        tap.set_armed(&[DeviceInstanceId(3)]);
        let on = NoteKind::On {
            key: 64,
            velocity: 0.5,
        };
        link.play_now((DeviceInstanceId(3), SocketId(0)), on)
            .unwrap();
        link.play_now(
            (DeviceInstanceId(3), SocketId(0)),
            NoteKind::Off { key: 64 },
        )
        .unwrap();
        link.play_now((DeviceInstanceId(4), SocketId(0)), on)
            .unwrap();
        link.send_now((DeviceInstanceId(3), SocketId(0)), on)
            .unwrap();
        let mut out = Vec::new();
        tap.drain(&mut out);
        assert_eq!(
            out.len(),
            2,
            "armed device only, audition not recorded: {out:?}"
        );
        assert_eq!(out[0].kind, on);
        assert!(matches!(out[1].kind, NoteKind::Off { key: 64 }));
        assert!(out[0].frame <= out[1].frame);
    }

    /// A message a mapping listens to is consumed (it plays no note, but the mappings still see
    /// it), every other message plays, running status survives a consumed message, and learning
    /// consumes everything.
    #[test]
    fn a_mapped_message_is_consumed_and_the_rest_plays() {
        use atomik_model::{MidiSource, SourceKind};
        let (link, tap, _target) = link_with_tap();
        let hub = Arc::clone(&link.hub);
        hub.set_consumed(vec![MidiSource::any_port(Some(0), SourceKind::Note(36))]);
        let mut fwd = PortInput::new("Pad", link);
        tap.set_armed(&[DeviceInstanceId(3)]);
        fwd.on_message(&[0x90, 36, 100]);
        fwd.on_message(&[0x80, 36, 0]);
        // Running status after a consumed message: the second note (37) is not mapped.
        fwd.on_message(&[0x90, 36, 100]);
        fwd.on_message(&[37, 90]);
        fwd.on_message(&[0x90, 60, 100]);
        let mut out = Vec::new();
        tap.drain(&mut out);
        let keys: Vec<u8> = out
            .iter()
            .filter_map(|n| match n.kind {
                NoteKind::On { key, .. } => Some(key),
                _ => None,
            })
            .collect();
        assert_eq!(keys, [37, 60], "only the unmapped keys play: {out:?}");
        // The mappings saw all of them, mapped or not.
        let mut seen = Vec::new();
        hub.drain(&mut seen);
        assert_eq!(seen.len(), 5);
        assert_eq!(&*seen[0].port, "Pad");
        // While learning nothing plays.
        out.clear();
        hub.set_learning(true);
        fwd.on_message(&[0x90, 61, 100]);
        tap.drain(&mut out);
        assert!(out.is_empty(), "{out:?}");
        hub.set_learning(false);
        fwd.on_message(&[0x90, 62, 100]);
        out.clear();
        tap.drain(&mut out);
        assert_eq!(out.len(), 1);
    }

    /// Two ports feed one target through one link: both keyboards play the same device.
    #[test]
    fn two_ports_play_the_same_target() {
        let (link, tap, _target) = link_with_tap();
        tap.set_armed(&[DeviceInstanceId(3)]);
        let mut keys = PortInput::new("Keys", link.clone());
        let mut pads = PortInput::new("Pads", link);
        keys.on_message(&[0x90, 60, 100]);
        pads.on_message(&[0x99, 36, 90]);
        keys.on_message(&[0x80, 60, 0]);
        pads.on_message(&[0x89, 36, 0]);
        let mut out = Vec::new();
        tap.drain(&mut out);
        let kinds: Vec<NoteKind> = out.iter().map(|n| n.kind).collect();
        assert_eq!(kinds.len(), 4, "{out:?}");
        assert!(matches!(kinds[0], NoteKind::On { key: 60, .. }));
        assert!(matches!(kinds[1], NoteKind::On { key: 36, .. }));
        assert!(matches!(kinds[2], NoteKind::Off { key: 60 }));
        assert!(matches!(kinds[3], NoteKind::Off { key: 36 }));
        assert!(out.iter().all(|n| n.device == DeviceInstanceId(3)));
        assert!(
            out.windows(2).all(|w| w[0].frame <= w[1].frame),
            "frames never decrease across ports: {out:?}"
        );
    }

    /// A note-off goes where its note-on went, whichever port sends it and wherever the target
    /// has moved since.
    #[test]
    fn note_offs_follow_their_note_ons_across_ports() {
        let (link, tap, target) = link_with_tap();
        tap.set_armed(&[DeviceInstanceId(3), DeviceInstanceId(4)]);
        let mut keys = PortInput::new("Keys", link.clone());
        let mut pads = PortInput::new("Pads", link);
        keys.on_message(&[0x90, 60, 100]);
        target.set(Some((DeviceInstanceId(4), SocketId(9))));
        pads.on_message(&[0x90, 62, 100]);
        // The target moves again; each port releases on the device its own note-on played.
        target.set(Some((DeviceInstanceId(5), SocketId(9))));
        keys.on_message(&[0x80, 60, 0]);
        pads.on_message(&[0x80, 62, 0]);
        let mut out = Vec::new();
        tap.drain(&mut out);
        let seen: Vec<(u32, NoteKind)> = out.iter().map(|n| (n.device.0, n.kind)).collect();
        assert_eq!(seen.len(), 4, "{seen:?}");
        assert_eq!((seen[2].0, seen[3].0), (3, 4), "{seen:?}");
        assert!(matches!(seen[2].1, NoteKind::Off { key: 60 }));
        assert!(matches!(seen[3].1, NoteKind::Off { key: 62 }));
    }

    /// A port that only feeds the mappings plays nothing, but the mappings still see it and
    /// its running status stays right.
    #[test]
    fn a_mapping_only_port_feeds_the_mappings_and_plays_nothing() {
        let (link, tap, _target) = link_with_tap();
        tap.set_armed(&[DeviceInstanceId(3)]);
        let hub = Arc::clone(&link.hub);
        let mut pads = PortInput::new("Pads", link.with_plays_notes(false));
        let mut keys = PortInput::new("Keys", link);
        pads.on_message(&[0x90, 36, 100]);
        pads.on_message(&[37, 100]);
        keys.on_message(&[0x90, 60, 100]);
        let mut out = Vec::new();
        tap.drain(&mut out);
        assert_eq!(out.len(), 1, "only the keyboard plays: {out:?}");
        let mut seen = Vec::new();
        hub.drain(&mut seen);
        assert_eq!(seen.len(), 3);
        assert_eq!(&*seen[0].port, "Pads");
        assert_eq!(&*seen[2].port, "Keys");
    }

    /// The feed never stamps a note before one already queued, and never past the ceiling, so
    /// a stamp far in the future cannot hold back the notes behind it.
    #[test]
    fn a_note_stamped_far_ahead_is_pulled_in_and_cannot_block_the_head() {
        let cfg = atomik_engine::EngineConfig {
            sample_rate: atomik_core::SampleRate::new(48_000.0).unwrap(),
            tempo: atomik_core::Tempo::new(120.0).unwrap(),
            input_channels: 0,
            output_channels: 2,
            seed: 0,
        };
        let (_engine, mut handle) = atomik_engine::new_engine(cfg);
        let mut feed = LiveFeed::new(handle.live_notes().unwrap());
        // Ordered across sources: a late stamp from another port is moved up behind the queue.
        assert_eq!(feed.admit(1_000, Some(5_000)), 1_000);
        assert_eq!(feed.admit(900, Some(5_000)), 1_000);
        // Far in the future: bounded by the ceiling (the host frame plus two buffers).
        assert_eq!(feed.admit(10_000_000, Some(1_512)), 1_512);
        assert_eq!(feed.admit(1_300, Some(1_600)), 1_512);
        // Without a ceiling (the clock has not published) the order is still kept.
        assert_eq!(feed.admit(1_000, None), 1_512);
        let note = ScheduledNote {
            frame: 99_999_999,
            device: DeviceInstanceId(1),
            socket: SocketId(0),
            kind: NoteKind::AllOff,
        };
        feed.push(note, Some(2_000)).unwrap();
        assert_eq!(feed.last_frame, 2_000);
    }

    /// The link's ceiling is the host frame plus the time since it was published plus two
    /// buffers.
    #[test]
    fn the_ceiling_is_the_host_frame_plus_two_buffers() {
        let (link, _tap, _target) = link_with_tap();
        assert_eq!(link.ceiling(timing::now()), None, "no clock published yet");
        let at = timing::now();
        link.clock.publish(10_000, at);
        assert_eq!(link.ceiling(at), Some(10_000 + 2 * 256));
        let later = at + Duration::from_millis(100);
        assert_eq!(link.ceiling(later), Some(10_000 + 4_800 + 2 * 256));
        // A note pushed through the link is pulled in as well.
        let note = ScheduledNote {
            frame: u64::MAX / 2,
            device: DeviceInstanceId(3),
            socket: SocketId(9),
            kind: NoteKind::AllOff,
        };
        link.push(note).unwrap();
        let queued = link.feed.lock().unwrap().last_frame;
        assert!(queued < 10_000 + 2 * 256 + 48_000 * 60, "{queued}");
    }
}
