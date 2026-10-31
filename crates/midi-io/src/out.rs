//! Timed MIDI output: a scheduled queue per port, and the thread that drains the MIDI taps of the
//! devices into it.
//!
//! The audio engine produces MIDI ahead of time, stamped with the engine host frame each message
//! belongs to ([`atomik_device_sdk::MidiTap`]). This module turns frames into wall-clock times
//! and sends every message to its port when its time comes. midir has no timestamps, so the
//! timing is ours: a thread polls the taps every millisecond, sleeps until a message is nearly
//! due and spins for the last stretch. It is the one place that decides *when* a MIDI message
//! leaves the program, and everything that sends MIDI on a schedule (the MIDI Out device, and
//! the MIDI clock through [`Core::set_sync`]) goes through it.
//!
//! # Pieces
//!
//! * [`frame_due`] / [`Timeline`]: the frame-to-time conversion, from the `(frame, instant)` pair
//!   the audio callback publishes ([`atomik_engine::FrameClock`]) plus the output latency, so a
//!   message is sent when its frame is *heard*, not when it was rendered.
//! * [`Scheduler`]: one time-ordered queue per port, the open ports, and a record of which notes
//!   each source has sounding, so a source that disappears (a device deleted, its port changed)
//!   can be silenced by the scheduler itself. Pure and single-threaded: tests drive it with
//!   synthetic instants and a fake port.
//! * [`Core`]: the scheduler plus the routing of taps to ports; [`Core::step`] is one pass of the
//!   thread (also pure).
//! * [`OutputThread`]: the thread around a [`Core`].
//!
//! # Jitter
//!
//! A message leaves within about one poll interval ([`POLL`], 1 ms) of its time when it is
//! produced less than a poll ahead, and within a few tens of microseconds of its time otherwise
//! (the thread spins for the last [`SPIN`]). The operating system's scheduler can add more, and
//! the port's driver adds its own latency (class-compliant USB devices typically 1 to 3 ms). The
//! thread runs at normal priority: it sleeps almost all of the time.

use crate::{MidiError, ports};
use atomik_device_sdk::MidiTap;
use atomik_engine::FrameClock;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the thread looks at the taps, at most.
pub const POLL: Duration = Duration::from_millis(1);

/// The last stretch before a message's time that the thread spins instead of sleeping.
pub const SPIN: Duration = Duration::from_micros(300);

/// How long the thread sleeps when it has no tap at all.
const IDLE: Duration = Duration::from_millis(20);

/// How long a port that failed to open or to send is left alone before the next attempt.
pub const RETRY_AFTER: Duration = Duration::from_secs(1);

/// A message this much later than its time counts as late ([`OutStats::late`]).
pub const LATE_AFTER: Duration = Duration::from_millis(2);

/// Most messages one pass reads from one tap, so a flood cannot starve the sends.
const MAX_PER_PASS: usize = 2048;

/// An open MIDI output the scheduler sends to.
pub trait MidiOutPort: Send {
    /// Sends one message.
    ///
    /// # Errors
    /// [`MidiError::Backend`] when the port refuses it (the device was unplugged).
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError>;
}

impl MidiOutPort for ports::MidiSender {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        ports::MidiSender::send(self, bytes)
    }
}

/// Opens the output port called `name` (exact or unique case-insensitive substring).
pub type Opener = Arc<dyn Fn(&str) -> Result<Box<dyn MidiOutPort>, MidiError> + Send + Sync>;

/// The opener of the real system ports (midir).
pub fn system_opener() -> Opener {
    Arc::new(|name| Ok(Box::new(ports::open_output(name)?) as Box<dyn MidiOutPort>))
}

/// The wall-clock time at which engine host frame `frame` is heard: the published pair `anchor`
/// (the frame the audio callback started rendering and when it did) plus the output latency.
/// Frames before the anchor give times before it.
pub fn frame_due(
    anchor: (u64, Instant),
    frame: u64,
    sample_rate: f64,
    output_latency_frames: u64,
) -> Instant {
    let (anchor_frame, anchor_at) = anchor;
    let ahead = i128::from(frame) - i128::from(anchor_frame) + i128::from(output_latency_frames);
    let secs = ahead.unsigned_abs() as f64 / sample_rate.max(1.0);
    let span = Duration::try_from_secs_f64(secs).unwrap_or(Duration::ZERO);
    if ahead >= 0 {
        anchor_at.checked_add(span).unwrap_or(anchor_at)
    } else {
        anchor_at.checked_sub(span).unwrap_or(anchor_at)
    }
}

/// The frame-to-time conversion of a running stream (see [`frame_due`]).
#[derive(Debug, Clone)]
pub struct Timeline {
    clock: Arc<FrameClock>,
    sample_rate: f64,
    output_latency: Arc<AtomicU64>,
}

impl Timeline {
    /// A timeline over the stream's frame clock. `output_latency` holds the output latency in
    /// frames; the app updates it as the stream reports it.
    pub fn new(clock: Arc<FrameClock>, sample_rate: f64, output_latency: Arc<AtomicU64>) -> Self {
        Self {
            clock,
            sample_rate,
            output_latency,
        }
    }

    /// When `frame` is heard, or `None` before the audio callback published its clock.
    pub fn due(&self, frame: u64) -> Option<Instant> {
        let anchor = self.clock.read()?;
        let latency = self.output_latency.load(Ordering::Relaxed);
        Some(frame_due(anchor, frame, self.sample_rate, latency))
    }

    /// The sample rate frames are counted at.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
}

/// A message of one to three bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Message {
    len: u8,
    bytes: [u8; 3],
}

impl Message {
    fn new(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > 3 {
            return None;
        }
        let mut b = [0; 3];
        b[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            len: bytes.len() as u8,
            bytes: b,
        })
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Pending {
    due: Instant,
    seq: u64,
    source: u32,
    message: Message,
}

impl PartialOrd for Message {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Message {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.len, self.bytes).cmp(&(other.len, other.bytes))
    }
}

/// The notes a source has started and not yet released, one bit per (channel, key).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Sounding([u128; 16]);

impl Sounding {
    fn apply(&mut self, bytes: &[u8]) {
        let (status, key, velocity) = match *bytes {
            [s, k, v] => (s, k, v),
            _ => return,
        };
        let channel = usize::from(status & 0x0F);
        let bit = 1u128 << (key & 0x7F);
        match status & 0xF0 {
            0x90 if velocity > 0 => self.0[channel] |= bit,
            0x90 | 0x80 => self.0[channel] &= !bit,
            // All Notes Off and All Sound Off clear the channel.
            0xB0 if key == 123 || key == 120 => self.0[channel] = 0,
            _ => {}
        }
    }

    fn is_empty(&self) -> bool {
        self.0.iter().all(|c| *c == 0)
    }
}

/// What the scheduler did, for the status line and the diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutStats {
    /// Messages handed to a port.
    pub sent: u64,
    /// Messages sent more than [`LATE_AFTER`] after their time.
    pub late: u64,
    /// The largest lateness seen, in microseconds.
    pub max_late_us: u64,
    /// Messages dropped: no port, a port that would not open, or a refused send.
    pub dropped: u64,
    /// Ports that failed to open or to send.
    pub port_errors: u64,
}

struct PortState {
    port: Option<Box<dyn MidiOutPort>>,
    /// No attempt to open before this time.
    retry_at: Option<Instant>,
    queue: BinaryHeap<Reverse<Pending>>,
    sounding: BTreeMap<u32, Sounding>,
}

/// One time-ordered queue per port (see the module docs).
pub struct Scheduler {
    opener: Opener,
    ports: BTreeMap<String, PortState>,
    seq: u64,
    stats: OutStats,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("ports", &self.ports.keys().collect::<Vec<_>>())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl Scheduler {
    /// A scheduler that opens its ports with `opener`.
    pub fn new(opener: Opener) -> Self {
        Self {
            opener,
            ports: BTreeMap::new(),
            seq: 0,
            stats: OutStats::default(),
        }
    }

    /// What it has done so far.
    pub fn stats(&self) -> OutStats {
        self.stats
    }

    /// Queues `bytes` (1 to 3 bytes) for port `port`, to leave at `due`. `source` names who made
    /// the message (a device), so that [`Self::release_source`] can find its notes. Messages for
    /// one port leave in time order, and in the order they were queued among equal times.
    pub fn enqueue(&mut self, port: &str, source: u32, due: Instant, bytes: &[u8]) {
        let Some(message) = Message::new(bytes) else {
            self.stats.dropped += 1;
            return;
        };
        self.seq += 1;
        let pending = Pending {
            due,
            seq: self.seq,
            source,
            message,
        };
        self.ports
            .entry(port.to_owned())
            .or_insert_with(|| PortState {
                port: None,
                retry_at: None,
                queue: BinaryHeap::with_capacity(256),
                sounding: BTreeMap::new(),
            })
            .queue
            .push(Reverse(pending));
    }

    /// Sends everything due at `now` and returns the time of the earliest message still waiting.
    pub fn pump(&mut self, now: Instant) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        let opener = Arc::clone(&self.opener);
        let stats = &mut self.stats;
        for (name, state) in &mut self.ports {
            while let Some(Reverse(head)) = state.queue.peek().copied() {
                if head.due > now {
                    break;
                }
                state.queue.pop();
                if !ensure_open(name, state, &opener, now, stats) {
                    stats.dropped += 1;
                    continue;
                }
                send(
                    state,
                    head.source,
                    head.message.bytes(),
                    Some((head.due, now)),
                    stats,
                );
            }
            if let Some(Reverse(head)) = state.queue.peek() {
                next = Some(next.map_or(head.due, |n| n.min(head.due)));
            }
        }
        next
    }

    /// Silences `source` at once: drops its queued messages, sends a note off for each note it
    /// has sounding and All Notes Off on the channels those were on. A source with nothing
    /// sounding sends nothing.
    pub fn release_source(&mut self, source: u32, now: Instant) {
        let opener = Arc::clone(&self.opener);
        let stats = &mut self.stats;
        for (name, state) in &mut self.ports {
            let kept: Vec<Reverse<Pending>> = state
                .queue
                .drain()
                .filter(|Reverse(p)| p.source != source)
                .collect();
            state.queue = kept.into();
            let Some(sounding) = state.sounding.remove(&source).filter(|s| !s.is_empty()) else {
                continue;
            };
            if !ensure_open(name, state, &opener, now, stats) {
                continue;
            }
            for (channel, keys) in sounding.0.iter().enumerate() {
                if *keys == 0 {
                    continue;
                }
                for key in 0..128u8 {
                    if keys & (1u128 << key) != 0 {
                        send(state, source, &[0x80 | channel as u8, key, 0], None, stats);
                    }
                }
                send(state, source, &[0xB0 | channel as u8, 123, 0], None, stats);
            }
        }
    }

    /// Releases every source and closes every port.
    pub fn shutdown(&mut self, now: Instant) {
        let sources: Vec<u32> = self
            .ports
            .values()
            .flat_map(|p| p.sounding.keys().copied())
            .collect();
        for source in sources {
            self.release_source(source, now);
        }
        self.ports.clear();
    }
}

/// Opens the port of `state` when it is not open and its retry time has passed.
fn ensure_open(
    name: &str,
    state: &mut PortState,
    opener: &Opener,
    now: Instant,
    stats: &mut OutStats,
) -> bool {
    if state.port.is_some() {
        return true;
    }
    if state.retry_at.is_some_and(|t| now < t) {
        return false;
    }
    match opener(name) {
        Ok(port) => {
            state.port = Some(port);
            state.retry_at = None;
            true
        }
        Err(_) => {
            stats.port_errors += 1;
            state.retry_at = Some(now + RETRY_AFTER);
            false
        }
    }
}

fn send(
    state: &mut PortState,
    source: u32,
    bytes: &[u8],
    timing: Option<(Instant, Instant)>,
    stats: &mut OutStats,
) {
    let Some(port) = state.port.as_mut() else {
        stats.dropped += 1;
        return;
    };
    match port.send(bytes) {
        Ok(()) => {
            stats.sent += 1;
            state.sounding.entry(source).or_default().apply(bytes);
            if let Some((due, now)) = timing {
                let late = now.saturating_duration_since(due);
                if late > LATE_AFTER {
                    stats.late += 1;
                }
                stats.max_late_us = stats.max_late_us.max(late.as_micros() as u64);
            }
        }
        Err(_) => {
            // The device went away: close the port, try again later.
            stats.dropped += 1;
            stats.port_errors += 1;
            state.port = None;
            state.retry_at = Some(Instant::now_for_retry() + RETRY_AFTER);
        }
    }
}

/// Wall-clock access for the retry back-off only: it never reaches the engine or the samples.
trait NowForRetry {
    fn now_for_retry() -> Instant;
}

impl NowForRetry for Instant {
    #[allow(clippy::disallowed_methods)] // Justification: see trait docs (live output timing only).
    fn now_for_retry() -> Instant {
        Instant::now()
    }
}

/// The wall clock the thread runs on.
#[allow(clippy::disallowed_methods)] // Justification: live output timing; never reaches the engine.
pub fn now() -> Instant {
    Instant::now()
}

/// A tap, where its messages go and which source it is.
struct Route {
    tap: Arc<MidiTap>,
    /// The port its messages go to; `None` drops them.
    port: Option<String>,
}

/// The source id the MIDI clock messages are queued under (no device has it).
pub const SYNC_SOURCE: u32 = u32::MAX;

/// An output port the MIDI clock is sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPort {
    /// The port's name.
    pub name: String,
    /// Milliseconds the clock leaves this port later (negative: earlier, as far as the engine
    /// produced it ahead of time) to make up for a device that answers slower or faster than the
    /// others.
    pub offset_ms: i32,
}

/// The clock messages of the engine and the ports they go to.
struct SyncRoute {
    tap: Arc<MidiTap>,
    ports: Vec<SyncPort>,
}

/// `due` moved by `offset_ms` milliseconds, not before `earliest`.
fn shifted(due: Instant, offset_ms: i32, earliest: Instant) -> Instant {
    let by = Duration::from_millis(u64::from(offset_ms.unsigned_abs()));
    let at = if offset_ms >= 0 {
        due.checked_add(by).unwrap_or(due)
    } else {
        due.checked_sub(by).unwrap_or(due)
    };
    at.max(earliest)
}

/// The scheduler plus the routing of device taps to ports: one pass of the output thread.
pub struct Core {
    scheduler: Scheduler,
    timeline: Timeline,
    routes: BTreeMap<u32, Route>,
    sync: Option<SyncRoute>,
}

impl std::fmt::Debug for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core")
            .field("scheduler", &self.scheduler)
            .field("sources", &self.routes.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Core {
    /// A core that opens ports with `opener` and times messages on `timeline`.
    pub fn new(opener: Opener, timeline: Timeline) -> Self {
        Self {
            scheduler: Scheduler::new(opener),
            timeline,
            routes: BTreeMap::new(),
            sync: None,
        }
    }

    /// The scheduler (for item-level access such as its statistics).
    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    /// The scheduler, mutably: other producers of timed messages (a clock) queue through it.
    pub fn scheduler_mut(&mut self) -> &mut Scheduler {
        &mut self.scheduler
    }

    /// The taps to drain, by source id. A source that is no longer listed (its device is gone,
    /// or a new engine replaced the old one) is silenced; a tap kept keeps its route.
    pub fn set_taps(&mut self, taps: Vec<(u32, Arc<MidiTap>)>, now: Instant) {
        let gone: Vec<u32> = self
            .routes
            .iter()
            .filter(|(id, old)| {
                !taps
                    .iter()
                    .any(|(i, tap)| i == *id && Arc::ptr_eq(tap, &old.tap))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.scheduler.release_source(id, now);
            self.routes.remove(&id);
        }
        for (id, tap) in taps {
            self.routes.entry(id).or_insert(Route { tap, port: None });
        }
    }

    /// Where each source's messages go (`None`: nowhere, they are dropped). A source whose port
    /// changed is silenced first, so no note is left sounding on the port it leaves.
    pub fn set_ports(&mut self, ports: &BTreeMap<u32, Option<String>>, now: Instant) {
        let changed: Vec<u32> = self
            .routes
            .iter()
            .filter(|(id, route)| ports.get(id).cloned().flatten() != route.port)
            .map(|(id, _)| *id)
            .collect();
        for id in changed {
            self.scheduler.release_source(id, now);
            if let Some(route) = self.routes.get_mut(&id) {
                route.port = ports.get(&id).cloned().flatten();
            }
        }
    }

    /// [`Self::set_taps`] and [`Self::set_ports`] as one step, so a source that is new never has
    /// a moment without a port.
    pub fn set_routes(
        &mut self,
        taps: Vec<(u32, Arc<MidiTap>)>,
        ports: &BTreeMap<u32, Option<String>>,
        now: Instant,
    ) {
        self.set_taps(taps, now);
        self.set_ports(ports, now);
    }

    /// Where the engine's MIDI clock goes: the tap it writes and the ports it is sent to, each
    /// with its own offset. `None` (or no port) sends nothing and drops what is queued. The route
    /// stays as long as it is given, even while the engine's clock out is off, so a Stop sent
    /// when it is switched off still arrives.
    pub fn set_sync(&mut self, tap: Option<Arc<MidiTap>>, ports: Vec<SyncPort>, now: Instant) {
        let same = self
            .sync
            .as_ref()
            .zip(tap.as_ref())
            .is_some_and(|(s, t)| Arc::ptr_eq(&s.tap, t) && s.ports == ports);
        if same {
            return;
        }
        // What waits for the old ports (or for a replaced engine) is stale.
        self.scheduler.release_source(SYNC_SOURCE, now);
        self.sync = tap.map(|tap| SyncRoute { tap, ports });
    }

    /// One pass: moves what the taps hold into the queues, sends what is due and returns the
    /// time of the next message still waiting. Messages whose frame has no wall-clock time yet
    /// (the audio clock has not published) go out at once.
    pub fn step(&mut self, now: Instant) -> Option<Instant> {
        let mut lost = Vec::new();
        for (id, route) in &self.routes {
            if route.tap.take_release_lost() {
                lost.push(*id);
            }
        }
        for id in lost {
            // A note off was lost to a full queue: silence the source rather than leave a note.
            self.scheduler.release_source(id, now);
        }
        for (id, route) in &self.routes {
            for _ in 0..MAX_PER_PASS {
                let Some(event) = route.tap.pop() else { break };
                let Some(port) = route.port.as_deref() else {
                    self.scheduler.stats.dropped += 1;
                    continue;
                };
                let due = self.timeline.due(event.frame).unwrap_or(now);
                self.scheduler.enqueue(port, *id, due, event.bytes());
            }
        }
        if let Some(sync) = &self.sync {
            for _ in 0..MAX_PER_PASS {
                let Some(event) = sync.tap.pop() else { break };
                let due = self.timeline.due(event.frame).unwrap_or(now);
                for port in &sync.ports {
                    let at = shifted(due, port.offset_ms, now.min(due));
                    self.scheduler
                        .enqueue(&port.name, SYNC_SOURCE, at, event.bytes());
                }
            }
        }
        self.scheduler.pump(now)
    }

    /// Whether any tap still holds messages.
    pub fn has_backlog(&self) -> bool {
        self.routes.values().any(|r| !r.tap.is_empty())
            || self.sync.as_ref().is_some_and(|s| !s.tap.is_empty())
    }

    /// Releases every source and closes the ports.
    pub fn shutdown(&mut self, now: Instant) {
        self.routes.clear();
        self.sync = None;
        self.scheduler.shutdown(now);
    }
}

enum Control {
    Routes(Vec<(u32, Arc<MidiTap>)>, BTreeMap<u32, Option<String>>),
    Sync(Option<Arc<MidiTap>>, Vec<SyncPort>),
    Shutdown,
}

/// The thread that runs a [`Core`] (named `atomik-midi-out`). Dropping it silences every source,
/// closes the ports and joins the thread.
pub struct OutputThread {
    tx: Sender<Control>,
    handle: Option<std::thread::JoinHandle<()>>,
    stats: Arc<Mutex<OutStats>>,
}

impl std::fmt::Debug for OutputThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputThread").finish_non_exhaustive()
    }
}

impl OutputThread {
    /// Starts the thread.
    ///
    /// # Errors
    /// The error of the operating system when it cannot start a thread.
    pub fn spawn(opener: Opener, timeline: Timeline) -> std::io::Result<Self> {
        let (tx, rx) = channel();
        let stats = Arc::new(Mutex::new(OutStats::default()));
        let shared = Arc::clone(&stats);
        let handle = std::thread::Builder::new()
            .name("atomik-midi-out".into())
            .spawn(move || run(Core::new(opener, timeline), &rx, &shared))?;
        Ok(Self {
            tx,
            handle: Some(handle),
            stats,
        })
    }

    /// Replaces the set of taps the thread drains and where each one sends (see
    /// [`Core::set_routes`]).
    pub fn set_routes(&self, taps: Vec<(u32, Arc<MidiTap>)>, ports: BTreeMap<u32, Option<String>>) {
        let _ = self.tx.send(Control::Routes(taps, ports));
    }

    /// Replaces where the engine's MIDI clock goes (see [`Core::set_sync`]). While a clock route
    /// is set the thread polls every millisecond, so give it `None` when the clock is off.
    pub fn set_sync(&self, tap: Option<Arc<MidiTap>>, ports: Vec<SyncPort>) {
        let _ = self.tx.send(Control::Sync(tap, ports));
    }

    /// What the thread has done so far.
    pub fn stats(&self) -> OutStats {
        self.stats.lock().map(|s| *s).unwrap_or_default()
    }
}

impl Drop for OutputThread {
    fn drop(&mut self) {
        let _ = self.tx.send(Control::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Applies one control message; `false` when the thread is to stop.
fn apply(core: &mut Core, control: Control, now: Instant) -> bool {
    match control {
        Control::Routes(taps, ports) => core.set_routes(taps, &ports, now),
        Control::Sync(tap, ports) => core.set_sync(tap, ports, now),
        Control::Shutdown => {
            core.shutdown(now);
            return false;
        }
    }
    true
}

fn run(mut core: Core, rx: &Receiver<Control>, stats: &Mutex<OutStats>) {
    loop {
        let now = now();
        loop {
            match rx.try_recv() {
                Ok(control) => {
                    if !apply(&mut core, control, now) {
                        publish_stats(&core, stats);
                        return;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    core.shutdown(now);
                    publish_stats(&core, stats);
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        let next = core.step(now);
        if let Ok(mut s) = stats.try_lock() {
            *s = core.scheduler().stats();
        }
        let wait = if core.routes.is_empty() && core.sync.is_none() {
            IDLE
        } else {
            next.map_or(POLL, |t| t.saturating_duration_since(now).min(POLL))
        };
        if wait > SPIN {
            // Sleep (waking early for a control message), leaving the last stretch to the spin.
            match rx.recv_timeout(wait - SPIN) {
                Ok(control) => {
                    // The messages that follow are handled at the top of the next pass.
                    if !apply(&mut core, control, self::now()) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    core.shutdown(self::now());
                    return;
                }
            }
        } else {
            std::hint::spin_loop();
        }
    }
}

fn publish_stats(core: &Core, stats: &Mutex<OutStats>) {
    if let Ok(mut s) = stats.lock() {
        *s = core.scheduler().stats();
    }
}

#[cfg(test)]
mod tests;
