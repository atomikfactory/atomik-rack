//! Live MIDI input with a retargetable destination.
//!
//! `atomik_midi_io::open_input` binds one fixed `(device, socket)` target and consumes the
//! engine's single [`LiveNoteSender`], so it cannot follow the selected device. This module is
//! the app-side workaround (contract gap reported): it reuses midi-io's pure [`parse`] and
//! [`event_frame`] and connects midir itself. Each note is routed through a [`NoteRouter`] to
//! the target currently published in the shared [`NoteTarget`] cell, so one sender serves
//! every device. Closing the connection hands the sender back, so the MIDI port can change
//! without restarting the engine.

use crate::midi_control::ControlHub;
use crate::record::RecordTap;
use crate::target::{NoteRouter, NoteTarget, Target};
use atomik_core::NoteKind;
use atomik_engine::{EngineError, FrameClock, LiveNoteSender, ScheduledNote};
use atomik_midi_io::{MidiError, RunningStatus, event_frame, parse, parse_message};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

/// Everything a MIDI input needs to feed the engine.
#[derive(Debug)]
pub struct MidiLink {
    /// The engine's live note producer.
    pub sender: LiveNoteSender,
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
}

impl MidiLink {
    /// Sends one note to `target` as if it arrived now (audition): timestamped like
    /// live MIDI (frame clock plus one buffer of latency), on the same live queue.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`] when the live queue is full.
    pub fn send_now(&mut self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        let latency = self.latency.load(Ordering::Relaxed);
        let frame = self.clock.read().map_or(0, |(frame, at)| {
            event_frame(frame, at, timing::now(), self.sample_rate, latency)
        });
        self.sender.push(ScheduledNote {
            frame,
            device: target.0,
            socket: target.1,
            kind,
        })
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

/// An open MIDI input port feeding the engine.
pub struct MidiConnection {
    connection: midir::MidiInputConnection<Forwarder>,
    port: String,
    dropped: Arc<AtomicU64>,
    /// The link's slot, shared with the callback (audition sends through it).
    slot: Arc<Mutex<Option<MidiLink>>>,
}

impl std::fmt::Debug for MidiConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiConnection")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// State owned by the midir callback.
///
/// The link sits in a shared slot because midir drops the callback data when `connect`
/// fails, and the engine's single live sender must survive that. The slot is locked only by
/// the MIDI thread (per message) and by `open`/`close` on the UI thread, never by the audio
/// thread (is untouched).
struct Forwarder {
    forwarder: NoteForwarder,
    /// The parser of the control path (its own running status).
    control_state: RunningStatus,
    /// The port's name, shared with every message sent to the hub.
    port: Arc<str>,
    slot: Arc<Mutex<Option<MidiLink>>>,
    dropped: Arc<AtomicU64>,
}

fn take_link(slot: &Mutex<Option<MidiLink>>) -> Option<MidiLink> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}

impl Forwarder {
    fn on_message(&mut self, bytes: &[u8]) {
        let at = timing::now();
        let mut guard = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(link) = guard.as_mut() else {
            return;
        };
        let dropped = &self.dropped;
        let latency = link.latency.load(Ordering::Relaxed);
        let clock = link.clock.read();
        let sender = &mut link.sender;
        let tap = &link.tap;
        // The control path first: the mappings see every message, and a message a mapping
        // consumes (or any message while learning) plays no note.
        let consumed = parse_message(bytes, &mut self.control_state).is_some_and(|m| {
            link.hub.push(&self.port, m);
            link.hub.consumes(&self.port, &m)
        });
        self.forwarder.forward_with(
            bytes,
            at,
            clock,
            link.sample_rate,
            latency,
            &link.target,
            consumed,
            |note| {
                if sender.push(note).is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
                // Recording: kept only for armed devices; never allocates.
                tap.offer(&note);
            },
        );
    }
}

impl MidiConnection {
    /// Opens input port `name` (exact name or unique case-insensitive substring). On failure
    /// the link (and its sender) is handed back.
    pub fn open(name: &str, link: MidiLink) -> Result<Self, (MidiError, MidiLink)> {
        let mut input = match midir::MidiInput::new("atomik-rack") {
            Ok(i) => i,
            Err(e) => return Err((MidiError::Backend(e.to_string()), link)),
        };
        // SysEx, clock and active sensing are out of scope.
        input.ignore(midir::Ignore::All);
        let ports = input.ports();
        let names: Vec<String> = ports
            .iter()
            .map(|p| input.port_name(p).unwrap_or_default())
            .collect();
        let index = match match_port(&names, name) {
            Ok(i) => i,
            Err(e) => return Err((e, link)),
        };
        let dropped = Arc::new(AtomicU64::new(0));
        let slot = Arc::new(Mutex::new(Some(link)));
        let forwarder = Forwarder {
            forwarder: NoteForwarder::new(),
            control_state: RunningStatus::default(),
            port: Arc::from(names[index].as_str()),
            slot: Arc::clone(&slot),
            dropped: Arc::clone(&dropped),
        };
        match input.connect(
            &ports[index],
            "atomik-rack-input",
            |_stamp, bytes, fwd: &mut Forwarder| fwd.on_message(bytes),
            forwarder,
        ) {
            Ok(connection) => Ok(Self {
                connection,
                port: names[index].clone(),
                dropped,
                slot,
            }),
            Err(e) => {
                let error = MidiError::Backend(e.to_string());
                // midir dropped the forwarder; the link is still in the slot.
                match take_link(&slot) {
                    Some(link) => Err((error, link)),
                    None => unreachable!("only the dropped forwarder could have taken the link"),
                }
            }
        }
    }

    /// The connected port's full name.
    pub fn port_name(&self) -> &str {
        &self.port
    }

    /// Notes dropped because the live queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Sends a note now through the link ([`MidiLink::send_now`]). Locks the slot briefly
    /// (shared with the MIDI thread only, never the audio thread).
    ///
    /// # Errors
    /// [`EngineError::QueueFull`] when the live queue is full.
    pub fn send_now(&self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        slot.as_mut()
            .map_or(Ok(()), |link| link.send_now(target, kind))
    }

    /// Closes the port and hands the link back.
    pub fn close(self) -> MidiLink {
        let (_input, forwarder) = self.connection.close();
        take_link(&forwarder.slot).expect("the link stays in the slot while connected")
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

    fn link_with_tap() -> (
        Arc<Mutex<Option<MidiLink>>>,
        Arc<RecordTap>,
        Arc<NoteTarget>,
    ) {
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
        let link = MidiLink {
            sender: handle.live_notes().unwrap(),
            clock: Arc::new(FrameClock::new()),
            target: Arc::clone(&target),
            sample_rate: 48_000.0,
            latency: Arc::new(AtomicU64::new(256)),
            tap: Arc::clone(&tap),
            hub: Arc::new(ControlHub::new()),
        };
        (Arc::new(Mutex::new(Some(link))), tap, target)
    }

    #[test]
    fn notes_to_armed_devices_are_offered_to_the_recording_tap() {
        let (slot, tap, target) = link_with_tap();
        let mut fwd = Forwarder {
            forwarder: NoteForwarder::new(),
            control_state: RunningStatus::default(),
            port: Arc::from("Example"),
            slot: Arc::clone(&slot),
            dropped: Arc::new(AtomicU64::new(0)),
        };
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
        let mut guard = slot.lock().unwrap();
        let link = guard.as_mut().unwrap();
        assert!(
            link.send_now((DeviceInstanceId(3), SocketId(9)), NoteKind::Off { key: 1 })
                .is_ok()
        );
    }

    /// A message a mapping listens to is consumed (it plays no note, but the mappings still see
    /// it), every other message plays, running status survives a consumed message, and learning
    /// consumes everything.
    #[test]
    fn a_mapped_message_is_consumed_and_the_rest_plays() {
        use atomik_model::{MidiSource, SourceKind};
        let (slot, tap, _target) = link_with_tap();
        let hub = Arc::clone(&slot.lock().unwrap().as_ref().unwrap().hub);
        hub.set_consumed(vec![MidiSource::any_port(Some(0), SourceKind::Note(36))]);
        let mut fwd = Forwarder {
            forwarder: NoteForwarder::new(),
            control_state: RunningStatus::default(),
            port: Arc::from("Pad"),
            slot: Arc::clone(&slot),
            dropped: Arc::new(AtomicU64::new(0)),
        };
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
}
