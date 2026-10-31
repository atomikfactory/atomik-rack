//! RT song player: transport, sample-accurate notes
//! with loops, held-note safety, per-batch automation, the metronome, and the lock-free
//! [`TransportState`] publication. Runs on the audio thread at each batch boundary, after the
//! control messages and queued params; nothing here allocates, frees, locks or blocks.
//!
//! # Design
//!
//! * **Position.** An integer *song frame* `pos` (frames since tick 0 on the song timeline).
//!   A tick `t` maps to song frame `F(t) = round(tick_to_frame(t))` through the snapshot's
//!   tempo segments (or, with no song, a constant map at the engine tempo). Positions never
//!   accumulate fractional increments: a batch advances `pos` by exactly 64 (or wraps), so
//!   nothing drifts, and everything is a pure function of (snapshot, sample rate,
//!   control messages at their batch boundaries) — independent of host block size and workers.
//! * **Batch tick range.** A playing batch covers song frames `[pos, pos + 64)`, split at the
//!   loop end when it falls inside. Each track keeps a cursor into its sorted events and emits
//!   every event with `F(tick)` inside the segment at offset `F(tick) - segment start`.
//! * **Loop.** With the loop enabled and `pos <= LE < pos + remaining`, the segment stops at
//!   `LE`; at that offset every held key is released (explicit `Off`s), `pos` jumps to `LS`,
//!   cursors are re-seeked and `loop_count` increments. A position past `LE` plays on (no
//!   wrap); a loop shorter than a batch wraps several times per batch. When a batch ends
//!   exactly at `LE`, the wrap happens at offset 0 of the next batch (before the state is
//!   published).
//! * **Held notes.** One 128-bit set per track. An `On` sets its bit; an `Off` is emitted only
//!   for a held key (so locating into a note with chase off never sends its off). Stop and
//!   Locate while playing send `AllOff` to every track input and clear the sets; a loop wrap
//!   releases the held keys with explicit `Off`s; a snapshot swap transfers the set of an
//!   unchanged track (same device input, same content hash) wholesale. On a changed track it
//!   transfers the keys that the new content still covers at the position (see *Chase*; chase
//!   on or off, nothing is retriggered) and releases every other held key with `Off`s.
//! * **Chase.** A track with `SongTrack::chase` starts the notes that span a seek position `pos`
//!   (`F(start) < pos < F(end)`, strictly: a note starting at `pos` plays from the event list, one
//!   ending at `pos` is over) after every seek: Play (after the seek, at offset 0), Locate while
//!   playing (after the `AllOff`, at offset 0; a stopped Locate only seeks, Play chases) and a
//!   loop wrap (after the releases, at the wrap offset; the pending wrap at offset 0 of a batch
//!   included). Each chased `On` carries the note's own velocity, goes to the track's input and
//!   sets the key's held bit, so the note's regular `Off` releases it. Keys are emitted in
//!   ascending order per track, tracks in order, so the (offset, generation order) sort keeps
//!   the releases before the chased `On`s. The notes come from the compiled per-track index
//!   (per-key overlaps already resolved: at most one note per key covers a position), sorted by
//!   start tick with a prefix maximum of the end ticks; a position walks it backwards from the
//!   last note starting before it and stops when no earlier note can still sound. Frames are
//!   compared through `F`, so the index holds ticks and survives any tempo map or sample rate.
//!   A snapshot swap never chases: notes that newly cover the position (an edit under the
//!   playhead) are not started.
//! * **Controllers.** A track's pitch bend, aftertouch and CC lanes are expanded at compile time
//!   into discrete events (`controllers.rs`) that sit in the track's event list: the walk sends
//!   them at their frame like notes, with no state of its own. A seek also restores each lane's
//!   value at the position (the last event strictly before it; before the first event zero for
//!   pitch bend, aftertouch and the sustain pedal, nothing for other CCs), on every track whether
//!   it chases notes or not, and Stop and a Locate while playing return those resetting lanes to
//!   zero right after the `AllOff`.
//! * **Merge order.** Song events are generated per segment in track order, then sorted by
//!   (offset, generation order). The engine merges them with the scheduled and live queues by
//!   frame; on equal frames: song, then scheduled, then live.
//! * **Automation.** At each playing batch start (after a pending wrap), every lane is
//!   evaluated at the exact fractional batch-start tick and applied with the clamping
//!   `set_param`; nothing is applied while stopped.
//! * **Automation hold.** While a take records automation, the app names the lanes it writes
//!   ([`AutomationHold`], set through [`crate::EngineHandle::hold_automation`]): those lanes are
//!   skipped, so the value the user is moving wins over the recorded lane. The hold is a few
//!   atomics read once per batch; it is empty (and costs one load) the rest of the time.
//! * **Metronome.** Beats are the beats of the meter map (a quarter note in 4/4, an eighth in 6/8;
//!   the beat lines restart at every bar line), each at the frame of its tick, accented on the
//!   first beat of a bar. A click is a 30 ms decaying sine burst from a table computed when the
//!   engine is created (1760 Hz accent at full level, 1320 Hz at 0.6), added to interface
//!   outputs 1/2 times the gain, only while playing (stopping cuts it).
//! * **Pre-roll.** `PlayWithPreRoll` holds the position while a counter of frames (a whole
//!   number of batches, planned off the audio thread by `SongSnapshot::pre_roll`) runs down one
//!   batch at a time. The transport is not playing meanwhile: no notes, automation or audio, and
//!   devices see `playing` false. The click's onsets (`i * frames / beats`, the first of each
//!   bar accented) sound whatever the metronome switch says. At zero the batch that follows
//!   starts the song from the held position as a Play does (chase included), loop and all, so a
//!   loop is counted in once. Stop, or a plain Play, cancels it. The state carries the frames
//!   left and the beat of the bar being counted.
//! * **Tempo.** With a song loaded, the song's tempo map drives positions and `TimeInfo::tempo`
//!   (the tempo at the batch start, the instantaneous one inside a ramp) and `TimeInfo::time_signature`
//!   (the meter at the batch start); `set_tempo` only matters for song-less racks, where it
//!   drives the transport too (the tick is kept when it changes while playing).
//! * **Clock out.** With clock out on ([`crate::SyncOut`]) each playing batch also writes a MIDI
//!   clock for every 640 ticks (`F(640 k)` through the tempo map, so exact across ramps) into a
//!   lock-free queue stamped with engine frames; Start, Stop and Continue follow the changes of
//!   the transport, a Song Position Pointer a locate or a loop wrap. Stopped, the clock can run
//!   on at the tempo of the position. A batch that sends nothing costs one atomic load.
//! * **Following.** While the [`crate::ExternalClock`] is followed, the player replaces every
//!   tempo map with one constant segment at the clock's tempo (the position keeps its musical
//!   place when the tempo moves: the tick is kept, the frame recomputed), starts, stops and
//!   locates as the clock does, ignores the transport commands and the count-in, leaves the audio
//!   clips out (they are placed on the song's own tempo) and stops the transport, releasing every
//!   held note, when the clock count stops moving for half a second. The phase of the clock is
//!   not tracked: the song runs on the tempo measured, from the start the clock gave.

use crate::arena::Arena;
use crate::audio::{AudioRt, CompiledAudio, Renderer};
use crate::console::DelayLine;
use crate::graph::DeviceInstanceId;
use crate::seq::{
    ChaseNote, CompiledTrack, PreRoll, SongCurve, SongPoint, SongSnapshot, TrackCursor,
    TransportCommand, TransportState,
};
use crate::stream::{StreamRequest, StreamStats};
use crate::sync::{ClockOut, ExternalClock, SyncOut, clocks_in};
use atomik_core::{
    BATCH, Batch, LoopWrap, MeterMap, MeterSegment, NoteKind, PPQ, ParamId, Tempo, TempoMap,
    TempoSegment, Ticks,
};
use atomik_device_sdk::SocketId;
use rtrb::Producer;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

/// One song note event for the current batch.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SongEvent {
    pub(crate) offset: u16,
    /// Generation order (makes the sort stable without allocating).
    seq: u32,
    pub(crate) device: DeviceInstanceId,
    pub(crate) socket: SocketId,
    pub(crate) kind: NoteKind,
}

/// Most lanes [`AutomationHold`] holds at once.
pub const HOLD_CAPACITY: usize = 32;

/// The automation lanes the player must leave alone (see the module docs): `(device, param)`
/// pairs in a fixed array of atomics, written by the handle and read by the audio thread once
/// per batch. No allocation, lock or wait on either side; a read that races a write sees the
/// old or the new set of a lane, which only moves a lane's hold by one batch.
#[derive(Debug)]
pub(crate) struct AutomationHold {
    count: AtomicU32,
    slots: [AtomicU64; HOLD_CAPACITY],
}

impl AutomationHold {
    const HELD: u64 = 1 << 63;

    pub(crate) fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// Replaces the held set (`None` when `lanes` does not fit).
    pub(crate) fn set(&self, lanes: &[(DeviceInstanceId, ParamId)]) -> Option<()> {
        if lanes.len() > HOLD_CAPACITY {
            return None;
        }
        self.count.store(0, Ordering::Release);
        for (slot, (device, param)) in self.slots.iter().zip(lanes) {
            slot.store(
                Self::HELD | u64::from(device.0) << 32 | u64::from(param.0),
                Ordering::Relaxed,
            );
        }
        self.count.store(lanes.len() as u32, Ordering::Release);
        Some(())
    }

    /// Whether the lane of `param` on `device` is held (RT-safe).
    fn holds(&self, device: DeviceInstanceId, param: ParamId) -> bool {
        let n = self.count.load(Ordering::Acquire) as usize;
        let want = Self::HELD | u64::from(device.0) << 32 | u64::from(param.0);
        n > 0
            && self.slots[..n]
                .iter()
                .any(|s| s.load(Ordering::Relaxed) == want)
    }
}

/// Lock-free (seqlock) cell holding the latest [`TransportState`]; one writer (the audio
/// thread), any number of readers.
#[derive(Debug, Default)]
pub(crate) struct TransportCell {
    seq: AtomicU64,
    flags: AtomicU64,
    tick: AtomicU64,
    engine_frame: AtomicU64,
    loop_count: AtomicU64,
    song_frame: AtomicU64,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    pre_roll: AtomicU64,
}

/// The bits of the published pre-roll word that hold the frames left.
const PRE_ROLL_FRAMES_MASK: u64 = (1 << 48) - 1;

impl TransportCell {
    /// Publishes `s` (single writer; wait-free: a few stores).
    fn publish(&self, s: &TransportState) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq + 1, Ordering::Relaxed);
        fence(Ordering::Release); // the odd sequence is visible before the data changes
        let flags = u64::from(s.playing) | u64::from(s.loop_enabled) << 1;
        self.flags.store(flags, Ordering::Relaxed);
        self.tick.store(s.tick as u64, Ordering::Relaxed);
        self.engine_frame.store(s.engine_frame, Ordering::Relaxed);
        self.loop_count.store(s.loop_count, Ordering::Relaxed);
        self.song_frame.store(s.song_frame, Ordering::Relaxed);
        self.loop_start
            .store(s.loop_start as u64, Ordering::Relaxed);
        self.loop_end.store(s.loop_end as u64, Ordering::Relaxed);
        // The beat sits above the 48 bits of frames (a pre-roll is minutes at most).
        self.pre_roll.store(
            s.pre_roll_frames & PRE_ROLL_FRAMES_MASK | u64::from(s.pre_roll_beat) << 48,
            Ordering::Relaxed,
        );
        self.seq.store(seq + 2, Ordering::Release);
    }

    /// The latest state, or the default before the first publish. Lock-free: retries only
    /// while a publish (a few stores) is in progress.
    pub(crate) fn read(&self) -> TransportState {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before == 0 {
                return TransportState::default();
            }
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let flags = self.flags.load(Ordering::Relaxed);
            let pre_roll = self.pre_roll.load(Ordering::Relaxed);
            let s = TransportState {
                playing: flags & 1 != 0,
                tick: self.tick.load(Ordering::Relaxed) as i64,
                engine_frame: self.engine_frame.load(Ordering::Relaxed),
                loop_count: self.loop_count.load(Ordering::Relaxed),
                song_frame: self.song_frame.load(Ordering::Relaxed),
                loop_start: self.loop_start.load(Ordering::Relaxed) as i64,
                loop_end: self.loop_end.load(Ordering::Relaxed) as i64,
                loop_enabled: flags & 2 != 0,
                pre_roll_frames: pre_roll & PRE_ROLL_FRAMES_MASK,
                pre_roll_beat: (pre_roll >> 48) as u16,
            };
            fence(Ordering::Acquire); // the data loads happen before the re-check
            if self.seq.load(Ordering::Relaxed) == before {
                return s;
            }
            std::hint::spin_loop();
        }
    }
}

/// Appends song events within the preallocated buffer; overflow is dropped and counted.
struct Emitter<'a> {
    events: &'a mut Vec<SongEvent>,
    seq: &'a mut u32,
    dropped: &'a AtomicU64,
}

impl Emitter<'_> {
    fn push(
        &mut self,
        offset: usize,
        device: DeviceInstanceId,
        socket: SocketId,
        kind: NoteKind,
    ) -> bool {
        if self.events.len() == self.events.capacity() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.events.push(SongEvent {
            offset: offset.min(BATCH - 1) as u16,
            seq: *self.seq,
            device,
            socket,
            kind,
        });
        *self.seq += 1;
        true
    }

    /// Explicit `Off`s for every held key of every track (ascending keys), at `offset`.
    fn release(&mut self, tracks: &[CompiledTrack], cursors: &mut [TrackCursor], offset: usize) {
        for (t, c) in tracks.iter().zip(cursors) {
            self.release_track(t.device, t.socket, &mut c.held, offset);
        }
    }

    fn release_track(
        &mut self,
        device: DeviceInstanceId,
        socket: SocketId,
        held: &mut u128,
        offset: usize,
    ) {
        let mut keys = *held;
        while keys != 0 {
            let key = keys.trailing_zeros() as u8;
            keys &= keys - 1;
            if self.push(offset, device, socket, NoteKind::Off { key }) {
                *held &= !(1u128 << key);
            }
        }
    }

    /// `AllOff` to every track input at offset 0, then each resetting controller lane (pitch
    /// bend, aftertouch, the sustain pedal) back to zero so a bend or a held pedal does not
    /// stay on.
    fn all_off(&mut self, tracks: &[CompiledTrack], cursors: &mut [TrackCursor]) {
        for (t, c) in tracks.iter().zip(cursors) {
            if self.push(0, t.device, t.socket, NoteKind::AllOff) {
                c.held = 0;
            }
            for lane in t.ctl.iter().filter(|l| l.kind.resets()) {
                self.push(0, t.device, t.socket, lane.kind.note(0.0));
            }
        }
    }
}

/// Points every cursor at the first event at or after song frame `pos`.
fn seek(
    map: TempoMap<'_>,
    sr: f64,
    tracks: &[CompiledTrack],
    cursors: &mut [TrackCursor],
    pos: u64,
) {
    for (t, c) in tracks.iter().zip(cursors) {
        c.next = t
            .events
            .partition_point(|e| map.song_frame(e.tick, sr) < pos);
    }
}

/// The keys of `t` whose note spans song frame `pos` (`F(start) < pos < F(end)`), as a bit set;
/// `velocity[key]` receives each one's velocity. Per-key overlaps are resolved at compile time,
/// so at most one note per key covers a position. Walks the start-sorted notes backwards from
/// the last one starting before `pos` and stops once the prefix maximum of the end ticks says
/// that no earlier note is still sounding. No allocation.
fn covering(
    map: TempoMap<'_>,
    sr: f64,
    t: &CompiledTrack,
    pos: u64,
    velocity: &mut [f32; 128],
) -> u128 {
    let started = t
        .notes
        .partition_point(|n: &ChaseNote| map.song_frame(n.start, sr) < pos);
    let mut keys = 0u128;
    for i in (0..started).rev() {
        if map.song_frame(t.max_end[i], sr) <= pos {
            break;
        }
        let n = &t.notes[i];
        if map.song_frame(n.end, sr) > pos {
            keys |= 1u128 << n.key;
            velocity[usize::from(n.key)] = n.velocity;
        }
    }
    keys
}

/// Sends each controller lane of `t` its value at song frame `pos` at batch offset `offset`: the
/// last value set strictly before `pos` (one set at `pos` is played by the normal walk), and
/// before a lane's first point zero for the controllers that reset (pitch bend, aftertouch, the
/// sustain pedal) and nothing for the rest. No allocation.
fn chase_controllers(
    map: TempoMap<'_>,
    sr: f64,
    t: &CompiledTrack,
    pos: u64,
    offset: usize,
    em: &mut Emitter<'_>,
) {
    for lane in &t.ctl {
        let n = lane.ticks.partition_point(|&k| map.song_frame(k, sr) < pos);
        let value = match n.checked_sub(1) {
            Some(i) => lane.values[i],
            None if lane.kind.resets() => 0.0,
            None => continue,
        };
        em.push(offset, t.device, t.socket, lane.kind.note(value));
    }
}

/// Starts the notes spanning song frame `pos` on every chasing track: an `On` at batch offset
/// `offset` per covered key (ascending keys, tracks in order) with the note's own velocity,
/// setting the key's held bit so the note's regular `Off` releases it. Every track first gets
/// its controller values ([`chase_controllers`]), chasing or not. Call after the seek (and after
/// the releases of a wrap), so these events sort after them.
fn chase(
    map: TempoMap<'_>,
    sr: f64,
    tracks: &[CompiledTrack],
    cursors: &mut [TrackCursor],
    pos: u64,
    offset: usize,
    em: &mut Emitter<'_>,
) {
    let mut velocity = [0.0f32; 128];
    for (t, c) in tracks.iter().zip(cursors) {
        chase_controllers(map, sr, t, pos, offset, em);
        if !t.chase {
            continue;
        }
        let mut keys = covering(map, sr, t, pos, &mut velocity);
        while keys != 0 {
            let key = keys.trailing_zeros() as u8;
            keys &= keys - 1;
            let kind = NoteKind::On {
                key,
                velocity: velocity[usize::from(key)],
            };
            if em.push(offset, t.device, t.socket, kind) {
                c.held |= 1u128 << key;
            }
        }
    }
}

/// A loop wrap at batch offset `at`: releases every held key, seeks to `ls` and chases the
/// notes spanning it.
fn wrap(
    map: TempoMap<'_>,
    sr: f64,
    parts: &mut Option<(&[CompiledTrack], &mut [TrackCursor])>,
    em: &mut Emitter<'_>,
    at: usize,
    ls: u64,
) {
    if let Some((tracks, cursors)) = parts.as_mut() {
        em.release(tracks, cursors, at);
        seek(map, sr, tracks, cursors, ls);
        chase(map, sr, tracks, cursors, ls, at, em);
    }
}

/// Emits the events of song frames `[a, b)` at batch offsets starting at `base`.
#[allow(clippy::too_many_arguments)] // Justification: one hot RT helper; a struct would only rename them.
fn emit_notes(
    map: TempoMap<'_>,
    sr: f64,
    tracks: &[CompiledTrack],
    cursors: &mut [TrackCursor],
    a: u64,
    b: u64,
    base: usize,
    em: &mut Emitter<'_>,
) {
    for (t, c) in tracks.iter().zip(cursors) {
        while let Some(ev) = t.events.get(c.next) {
            let f = map.song_frame(ev.tick, sr);
            if f >= b {
                break;
            }
            let offset = base + f.saturating_sub(a) as usize;
            match ev.kind {
                NoteKind::On { key, .. } => {
                    if em.push(offset, t.device, t.socket, ev.kind) {
                        c.held |= 1u128 << key;
                    }
                }
                NoteKind::Off { key } => {
                    let bit = 1u128 << key;
                    if c.held & bit != 0 && em.push(offset, t.device, t.socket, ev.kind) {
                        c.held &= !bit;
                    }
                }
                // The controller lanes: sent as they come, they hold no state in the cursor.
                NoteKind::PitchBend { .. }
                | NoteKind::Cc { .. }
                | NoteKind::ChannelPressure { .. }
                | NoteKind::PolyPressure { .. } => {
                    em.push(offset, t.device, t.socket, ev.kind);
                }
                // A song track never holds `AllOff` or per-note expressions.
                NoteKind::AllOff | NoteKind::NoteExpr { .. } => {}
            }
            c.next += 1;
        }
    }
}

/// The lane's value at a fractional tick: held before the first and after the last point,
/// linear or step in between (equal ticks: the last point wins, a jump).
pub fn lane_value(points: &[SongPoint], tick: f64) -> f32 {
    let k = points.partition_point(|p| p.tick.0 as f64 <= tick);
    let Some(p) = k.checked_sub(1).and_then(|i| points.get(i)) else {
        return points.first().map_or(0.0, |p| p.value);
    };
    match (p.curve, points.get(k)) {
        (SongCurve::Linear, Some(q)) => {
            let span = (q.tick.0 - p.tick.0) as f64;
            let x = (tick - p.tick.0 as f64) / span;
            (f64::from(p.value) + (f64::from(q.value) - f64::from(p.value)) * x) as f32
        }
        _ => p.value,
    }
}

/// Click length and shapes.
const CLICK_SECONDS: f64 = 0.03;
const CLICK_DECAY_SECONDS: f64 = 0.006;
const CLICK_ATTACK_SECONDS: f64 = 0.000_5;

fn click_table(sr: f64, hz: f64, level: f64) -> Box<[f32]> {
    let len = (CLICK_SECONDS * sr).round().max(1.0) as usize;
    let attack = CLICK_ATTACK_SECONDS * sr;
    (0..len)
        .map(|n| {
            let n = n as f64;
            let env = (-n / (CLICK_DECAY_SECONDS * sr)).exp() * (n / attack).min(1.0);
            (level * env * (std::f64::consts::TAU * hz * n / sr).sin()) as f32
        })
        .collect()
}

/// Hands each audio track's rendered playback to its device and records where each stream
/// lane's playback ended.
fn publish_audio(arena: &mut Arena, tracks: &[CompiledAudio], rts: &mut [AudioRt], pos: u64) {
    for (t, rt) in tracks.iter().zip(rts.iter_mut()) {
        arena.set_playback(t.device, &rt.pb);
        if let Some(lane) = rt.stream.as_mut() {
            lane.end_batch(pos);
        }
    }
}

/// What devices see of the transport in one batch: the batch tempo and position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BatchTime {
    pub(crate) tempo: Tempo,
    /// Transport tick at the first frame of the batch.
    pub(crate) tick: f64,
    /// Ticks per frame at `tempo`.
    pub(crate) ticks_per_frame: f64,
    /// The loop wrap inside the batch, if any (the first one).
    pub(crate) loop_wrap: Option<LoopWrap>,
    /// The enabled loop region in ticks (only with a song).
    pub(crate) loop_ticks: Option<(f64, f64)>,
    /// `(numerator, denominator)`: the meter at the batch start; 4/4 without a song.
    pub(crate) time_signature: (u8, u8),
}

/// A count-in in progress: the plan and the frames left.
#[derive(Debug, Clone, Copy)]
struct Counting {
    plan: PreRoll,
    remaining: u64,
}

/// The player is following an external clock (see the module docs, *Following*).
#[derive(Debug, Clone, Copy)]
struct Follow {
    /// The tempo the song plays at: the external clock's, constant between two batches.
    tempo: Tempo,
    /// The locates of the clock already applied.
    locates: u64,
    /// The clock count at the last batch, and the frames since it last moved.
    clocks: u64,
    quiet_frames: u64,
}

/// The clock may be silent this long (in seconds) while the sender runs before the transport
/// stops: more than one clock at the slowest tempo (20 BPM: 125 ms).
const CLOCK_LOST_SECONDS: f64 = 0.5;

/// The RT player state (owned by the engine).
pub(crate) struct Player {
    song: Option<Box<SongSnapshot>>,
    playing: bool,
    /// Song frame at the start of the next batch.
    pos: u64,
    loop_count: u64,
    loop_start: i64,
    loop_end: i64,
    loop_enabled: bool,
    metronome: bool,
    gain: f32,
    /// The count-in that runs before the song starts, if any.
    pre_roll: Option<Counting>,
    /// Whether this batch's click onsets are a count-in's (they sound with the metronome off).
    counting: bool,
    /// Tick of the song-less transport at the start of the next batch: it advances with the
    /// engine at the engine tempo, so it stays continuous across tempo changes.
    free_tick: f64,
    /// Song events of the current batch (capacity `SONG_EVENT_CAPACITY`), sorted before
    /// delivery; `next_event` is the delivery cursor.
    pub(crate) events: Vec<SongEvent>,
    pub(crate) next_event: usize,
    seq: u32,
    /// Click onsets of the current batch: (offset, accent).
    clicks: [(u16, bool); BATCH + 1],
    click_count: usize,
    /// The sounding click: (table index, accent).
    click: Option<(usize, bool)>,
    accent: Box<[f32]>,
    beat: Box<[f32]>,
    shared: Arc<TransportCell>,
    /// The lanes a take is writing; the player skips them.
    hold: Arc<AutomationHold>,
    /// MIDI clock out.
    sync: ClockOut,
    /// The clock source the player follows while the cell says so.
    external: Arc<ExternalClock>,
    follow: Option<Follow>,
    /// Refill requests to the streamer.
    requests: Producer<StreamRequest>,
    /// Stream counters shared with the handle.
    stats: Arc<StreamStats>,
}

/// Non-RT construction.
impl Player {
    pub(crate) fn new(
        sample_rate: f64,
        requests: Producer<StreamRequest>,
        stats: Arc<StreamStats>,
    ) -> Self {
        Self {
            song: None,
            playing: false,
            pos: 0,
            loop_count: 0,
            loop_start: 0,
            loop_end: 0,
            loop_enabled: false,
            metronome: false,
            gain: 0.0,
            pre_roll: None,
            counting: false,
            free_tick: 0.0,
            events: Vec::with_capacity(crate::engine::SONG_EVENT_CAPACITY),
            next_event: 0,
            seq: 0,
            clicks: [(0, false); BATCH + 1],
            click_count: 0,
            click: None,
            accent: click_table(sample_rate, 1760.0, 1.0),
            beat: click_table(sample_rate, 1320.0, 0.6),
            shared: Arc::new(TransportCell::default()),
            hold: Arc::new(AutomationHold::new()),
            sync: ClockOut::new(),
            external: ExternalClock::new(),
            follow: None,
            requests,
            stats,
        }
    }

    /// The clock-out settings and queue the handle shares.
    pub(crate) fn sync_out(&self) -> Arc<SyncOut> {
        self.sync.shared()
    }

    /// The cell a clock input writes and the handle switches.
    pub(crate) fn external(&self) -> Arc<ExternalClock> {
        Arc::clone(&self.external)
    }

    /// The hold the handle writes the lanes to skip to.
    pub(crate) fn hold(&self) -> Arc<AutomationHold> {
        Arc::clone(&self.hold)
    }

    /// The cell the handle reads the transport state from.
    pub(crate) fn shared(&self) -> Arc<TransportCell> {
        Arc::clone(&self.shared)
    }
}

/// Real-time side. Nothing here allocates, frees, locks or blocks.
impl Player {
    /// Starts a batch: the event buffer is emptied (control messages may add releases).
    pub(crate) fn begin_batch(&mut self) {
        self.events.clear();
        self.next_event = 0;
        self.seq = 0;
    }

    /// Whether devices see `TimeInfo::playing` (always true without a song, as before).
    pub(crate) fn time_playing(&self) -> bool {
        self.song.is_none() || self.playing
    }

    /// Swaps in a new snapshot; returns the old one (to be handed back, never dropped here).
    pub(crate) fn swap_song(
        &mut self,
        mut new: Box<SongSnapshot>,
        engine_tempo: Tempo,
        sr: f64,
        dropped: &AtomicU64,
    ) -> Option<Box<SongSnapshot>> {
        let constant = [TempoSegment::constant(engine_tempo)];
        let old = self.song.take();
        // While following, the position is on the external clock's constant tempo, whatever the
        // snapshots' own maps say.
        let ext = self.follow.map(|f| [TempoSegment::constant(f.tempo)]);
        let old_map = match ext.as_ref() {
            Some(e) => TempoMap(e),
            None => old
                .as_deref()
                .map_or(TempoMap(&constant), SongSnapshot::map),
        };
        let new_map = match ext.as_ref() {
            Some(e) => TempoMap(e),
            None => TempoMap(&new.tempo),
        };
        if old_map.0 != new_map.0 {
            // Keep the musical position (the tick), not the frame.
            let tick = old_map.tick(self.pos as f64, sr);
            self.pos = new_map.frame(tick, sr).round().max(0.0) as u64;
        }
        if let Some(old) = old.as_deref() {
            let mut em = Emitter {
                events: &mut self.events,
                seq: &mut self.seq,
                dropped,
            };
            for (t, c) in old.tracks.iter().zip(&old.cursors) {
                let mut held = c.held;
                if held == 0 {
                    continue;
                }
                let key = (t.device, t.socket);
                if let Ok(i) = new.by_key.binary_search_by(|e| (e.0, e.1).cmp(&key)) {
                    let j = new.by_key[i].2;
                    if new.tracks[j].hash == t.hash && new.tracks[j].events.len() == t.events.len()
                    {
                        new.cursors[j].held = held;
                        continue;
                    }
                    // A changed track keeps the keys its new content still covers at the
                    // position (no retrigger, chase or not); the rest is released below.
                    let mut velocity = [0.0f32; 128];
                    let covered = covering(new_map, sr, &new.tracks[j], self.pos, &mut velocity);
                    new.cursors[j].held = held & covered;
                    held &= !covered;
                }
                em.release_track(t.device, t.socket, &mut held, 0);
            }
        }
        let SongSnapshot {
            tempo,
            tracks,
            cursors,
            ..
        } = &mut *new;
        let map = ext.as_ref().map_or(TempoMap(tempo), |e| TempoMap(e));
        seek(map, sr, tracks, cursors, self.pos);
        self.song = Some(new);
        old
    }

    /// Applies a transport command at this batch boundary. While following an external clock the
    /// transport is the clock's: Play, Stop, Locate and a count-in are ignored.
    pub(crate) fn command(
        &mut self,
        command: TransportCommand,
        engine_tempo: Tempo,
        sr: f64,
        dropped: &AtomicU64,
    ) {
        if self.follow.is_some()
            && matches!(
                command,
                TransportCommand::Play
                    | TransportCommand::PlayWithPreRoll(_)
                    | TransportCommand::Stop
                    | TransportCommand::Locate(_)
            )
        {
            return;
        }
        self.apply_command(command, engine_tempo, sr, dropped);
    }

    fn apply_command(
        &mut self,
        command: TransportCommand,
        engine_tempo: Tempo,
        sr: f64,
        dropped: &AtomicU64,
    ) {
        let constant = [TempoSegment::constant(engine_tempo)];
        let ext = self.follow.map(|f| [TempoSegment::constant(f.tempo)]);
        let Self {
            song,
            playing,
            pos,
            events,
            seq,
            ..
        } = self;
        let (map, mut parts) = match song.as_deref_mut() {
            Some(s) => (
                ext.as_ref().map_or(TempoMap(&s.tempo), |e| TempoMap(e)),
                Some((&s.tracks[..], &mut s.cursors[..])),
            ),
            None => (
                ext.as_ref().map_or(TempoMap(&constant), |e| TempoMap(e)),
                None,
            ),
        };
        let mut em = Emitter {
            events,
            seq,
            dropped,
        };
        match command {
            TransportCommand::Play => {
                self.pre_roll = None;
                if !*playing {
                    *playing = true;
                    if let Some((tracks, cursors)) = parts.as_mut() {
                        seek(map, sr, tracks, cursors, *pos);
                        chase(map, sr, tracks, cursors, *pos, 0, &mut em);
                    }
                }
            }
            TransportCommand::PlayWithPreRoll(plan) => {
                // Already playing: nothing to count in. The song starts when the count ends
                // (`run`), so a Stop or a Play in between cancels it.
                if !*playing {
                    self.pre_roll = (plan.frames > 0 && plan.beats > 0).then_some(Counting {
                        plan,
                        remaining: plan.frames.div_ceil(BATCH as u64) * BATCH as u64,
                    });
                    if self.pre_roll.is_none() {
                        *playing = true;
                        if let Some((tracks, cursors)) = parts.as_mut() {
                            seek(map, sr, tracks, cursors, *pos);
                            chase(map, sr, tracks, cursors, *pos, 0, &mut em);
                        }
                    }
                }
            }
            TransportCommand::Stop => {
                self.pre_roll = None;
                self.click = None;
                if *playing {
                    *playing = false;
                    if let Some((tracks, cursors)) = parts.as_mut() {
                        em.all_off(tracks, cursors);
                    }
                }
            }
            TransportCommand::Locate(tick) => {
                self.sync.located(tick.0);
                if let Some((tracks, cursors)) = parts.as_mut() {
                    if *playing {
                        em.all_off(tracks, cursors);
                    }
                    *pos = map.song_frame(tick.0, sr);
                    seek(map, sr, tracks, cursors, *pos);
                    // While stopped, Play chases from the new position instead.
                    if *playing {
                        chase(map, sr, tracks, cursors, *pos, 0, &mut em);
                    }
                } else {
                    *pos = map.song_frame(tick.0, sr);
                }
            }
            TransportCommand::SetLoop {
                start,
                end,
                enabled,
            } => {
                self.loop_start = start.0;
                self.loop_end = end.0;
                self.loop_enabled = enabled;
            }
            TransportCommand::Metronome { enabled, gain } => {
                self.metronome = enabled;
                self.gain = if gain.is_finite() {
                    gain.clamp(0.0, 1.0)
                } else {
                    0.0
                };
            }
        }
    }

    /// Follows the external clock at a batch boundary (see the module docs): takes the clock's
    /// tempo, applies its locates and its start and stop, and stops the transport when the
    /// clock disappears. Switching following on or off stops the transport first.
    fn poll_external(&mut self, sr: f64, engine_tempo: Tempo, dropped: &AtomicU64) {
        match (self.external.following(), self.follow) {
            (false, None) => {}
            (true, None) => self.enter_follow(sr, engine_tempo, dropped),
            (false, Some(_)) => self.leave_follow(sr, engine_tempo, dropped),
            (true, Some(f)) => self.follow_step(f, sr, engine_tempo, dropped),
        }
    }

    /// The tick of the position and the tempo there, on the song's own map (or the engine's).
    fn own_tempo_at_position(&self, sr: f64, engine_tempo: Tempo) -> (f64, Tempo) {
        let constant = [TempoSegment::constant(engine_tempo)];
        let map = self
            .song
            .as_deref()
            .map_or(TempoMap(&constant), SongSnapshot::map);
        let tick = map.tick(self.pos as f64, sr);
        (tick, Tempo::new(map.bpm(tick)).unwrap_or(engine_tempo))
    }

    fn enter_follow(&mut self, sr: f64, engine_tempo: Tempo, dropped: &AtomicU64) {
        let state = self.external.read();
        self.apply_command(TransportCommand::Stop, engine_tempo, sr, dropped);
        let (tick, own) = self.own_tempo_at_position(sr, engine_tempo);
        let tempo = state
            .tempo
            .and_then(|bpm| Tempo::new(bpm.clamp(20.0, 999.0)).ok())
            .unwrap_or(own);
        self.follow = Some(Follow {
            tempo,
            locates: state.locates,
            clocks: state.clocks,
            quiet_frames: 0,
        });
        // The same musical position on the clock's tempo, with the cursors moved to it.
        self.apply_command(
            TransportCommand::Locate(Ticks(tick.round() as i64)),
            engine_tempo,
            sr,
            dropped,
        );
    }

    fn leave_follow(&mut self, sr: f64, engine_tempo: Tempo, dropped: &AtomicU64) {
        self.apply_command(TransportCommand::Stop, engine_tempo, sr, dropped);
        let tempo = self.follow.map_or(engine_tempo, |f| f.tempo);
        let tick = TempoMap(&[TempoSegment::constant(tempo)]).tick(self.pos as f64, sr);
        self.follow = None;
        self.apply_command(
            TransportCommand::Locate(Ticks(tick.round() as i64)),
            engine_tempo,
            sr,
            dropped,
        );
    }

    fn follow_step(&mut self, mut f: Follow, sr: f64, engine_tempo: Tempo, dropped: &AtomicU64) {
        let s = self.external.read();
        if let Some(t) = s
            .tempo
            .and_then(|bpm| Tempo::new(bpm.clamp(20.0, 999.0)).ok())
            .filter(|t| *t != f.tempo)
        {
            // The same musical position on the new tempo.
            let tick = TempoMap(&[TempoSegment::constant(f.tempo)]).tick(self.pos as f64, sr);
            self.pos = TempoMap(&[TempoSegment::constant(t)])
                .frame(tick, sr)
                .round()
                .max(0.0) as u64;
            f.tempo = t;
        }
        let locate = (s.locates != f.locates).then_some(s.position);
        f.locates = s.locates;
        if s.clocks == f.clocks {
            f.quiet_frames += BATCH as u64;
        } else {
            f.clocks = s.clocks;
            f.quiet_frames = 0;
        }
        let silent = f.quiet_frames as f64 > CLOCK_LOST_SECONDS * sr;
        self.follow = Some(f);
        if let Some(position) = locate {
            self.apply_command(
                TransportCommand::Locate(Ticks(position)),
                engine_tempo,
                sr,
                dropped,
            );
        }
        let running = self.playing || self.pre_roll.is_some();
        if running && (!s.running || silent) {
            // A lost clock is a stop (nothing is left sounding); the sender running again with
            // clocks coming continues it.
            self.apply_command(TransportCommand::Stop, engine_tempo, sr, dropped);
        } else if !running && s.running && !silent {
            self.apply_command(TransportCommand::Play, engine_tempo, sr, dropped);
        }
        self.follow = Some(f);
    }

    /// The engine tempo changed: a song-less transport keeps its tick.
    pub(crate) fn engine_tempo_changed(&mut self, old: Tempo, new: Tempo, sr: f64) {
        if self.song.is_none() && old != new {
            let tick = TempoMap(&[TempoSegment::constant(old)]).tick(self.pos as f64, sr);
            self.pos = TempoMap(&[TempoSegment::constant(new)])
                .frame(tick, sr)
                .round()
                .max(0.0) as u64;
        }
    }

    /// Plays one batch starting at engine frame `frame`: wraps a pending loop end, publishes
    /// the transport state, applies automation, emits the batch's notes and click onsets and
    /// advances the position. Returns the tempo and position devices see in this batch.
    pub(crate) fn run(
        &mut self,
        arena: &mut Arena,
        frame: u64,
        sr: f64,
        engine_tempo: Tempo,
        dropped: &AtomicU64,
    ) -> BatchTime {
        self.poll_external(sr, engine_tempo, dropped);
        let constant = [TempoSegment::constant(engine_tempo)];
        // Following an external clock: its constant tempo replaces every map, and the audio
        // clips (placed on the song's own tempo) are left out.
        let ext = self.follow.map(|f| [TempoSegment::constant(f.tempo)]);
        let Self {
            song,
            playing,
            pos,
            loop_count,
            events,
            seq,
            clicks,
            click_count,
            requests,
            stats,
            free_tick,
            hold,
            pre_roll,
            counting,
            sync,
            ..
        } = self;
        let four_four = [MeterSegment::FOUR_FOUR];
        let (map, meters, lanes, mut parts, mut aud) = match song.as_deref_mut() {
            Some(s) => (
                ext.as_ref().map_or(TempoMap(&s.tempo), |e| TempoMap(e)),
                MeterMap(&s.meter),
                &s.lanes[..],
                Some((&s.tracks[..], &mut s.cursors[..])),
                ext.is_none().then_some((&s.audio[..], &mut s.audio_rt[..])),
            ),
            None => (
                ext.as_ref().map_or(TempoMap(&constant), |e| TempoMap(e)),
                MeterMap(&four_four),
                &[][..],
                None,
                None,
            ),
        };
        let (ls, le) = (
            map.song_frame(self.loop_start, sr),
            map.song_frame(self.loop_end, sr),
        );
        let lp = (self.loop_enabled && le > ls).then_some((ls, le));
        let mut em = Emitter {
            events,
            seq,
            dropped,
        };
        if let Some((tracks, rts)) = aud.as_mut() {
            for (t, rt) in tracks.iter().zip(rts.iter_mut()) {
                if let Some(lane) = rt.stream.as_mut().filter(|_| t.streams) {
                    lane.sync(*pos, lp, requests, stats);
                }
            }
        }
        // The count-in ends on a batch boundary: the song starts here, from the held position.
        if pre_roll.is_some_and(|c| c.remaining == 0) {
            *pre_roll = None;
            *playing = true;
            if let Some((tracks, cursors)) = parts.as_mut() {
                seek(map, sr, tracks, cursors, *pos);
                chase(map, sr, tracks, cursors, *pos, 0, &mut em);
            }
        }
        // MIDI clock out: the transport changes since the last batch go out first.
        let sync_cfg = sync.batch_start(frame, *playing, map.tick(*pos as f64, sr).round() as i64);
        if *playing
            && let Some((ls, le)) = lp
            && *pos == le
        {
            wrap(map, sr, &mut parts, &mut em, 0, ls);
            *pos = ls;
            *loop_count += 1;
            sync.wrapped(frame, self.loop_start, sync_cfg);
        }
        self.shared.publish(&TransportState {
            playing: *playing,
            tick: map.tick(*pos as f64, sr).round() as i64,
            engine_frame: frame,
            loop_count: *loop_count,
            song_frame: *pos,
            loop_start: self.loop_start,
            loop_end: self.loop_end,
            loop_enabled: self.loop_enabled,
            pre_roll_frames: pre_roll.map_or(0, |c| c.remaining),
            pre_roll_beat: pre_roll.map_or(0, |c| {
                let elapsed = c.plan.frames.saturating_sub(c.remaining);
                let beats = u64::from(c.plan.beats.max(1));
                let beat = (elapsed * beats / c.plan.frames.max(1)).min(beats - 1);
                (beat % u64::from(c.plan.per_bar.max(1))) as u16 + 1
            }),
        });
        let tick0 = map.tick(*pos as f64, sr);
        let tempo = Tempo::new(map.bpm(tick0)).unwrap_or(engine_tempo);
        let ticks_per_frame = tempo.bpm() * PPQ as f64 / (60.0 * sr);
        // With a song the player's tick; without one the free-running engine tick.
        let tick = if parts.is_some() { tick0 } else { *free_tick };
        if parts.is_none() {
            *free_tick += BATCH as f64 * ticks_per_frame;
        }
        let mut time = BatchTime {
            tempo,
            tick,
            ticks_per_frame,
            loop_wrap: None,
            loop_ticks: (lp.is_some() && parts.is_some())
                .then_some((self.loop_start as f64, self.loop_end as f64)),
            time_signature: meters.meter_at(tick.floor() as i64),
        };
        *click_count = 0;
        *counting = false;
        if sync_cfg.enabled && sync_cfg.while_stopped && (pre_roll.is_some() || !*playing) {
            sync.idle(frame, BATCH, ticks_per_frame);
        }
        if let Some(c) = pre_roll.as_mut() {
            // Counting in: the position holds, nothing of the song plays, and the clicks that
            // fall in this batch are queued.
            *counting = true;
            let elapsed = c.plan.frames - c.remaining;
            let beats = u64::from(c.plan.beats);
            let mut i = (elapsed * beats).div_ceil(c.plan.frames.max(1));
            while i < beats {
                let at = c.plan.onset(i);
                if at >= elapsed + BATCH as u64 {
                    break;
                }
                if at >= elapsed && *click_count < clicks.len() {
                    let accent = i % u64::from(c.plan.per_bar.max(1)) == 0;
                    clicks[*click_count] = ((at - elapsed) as u16, accent);
                    *click_count += 1;
                }
                i += 1;
            }
            c.remaining -= BATCH as u64;
            return time;
        }
        if !*playing {
            return time;
        }
        for lane in lanes {
            if !hold.holds(lane.device, lane.param) {
                arena.set_param(lane.device, lane.param, lane_value(&lane.points, tick0));
            }
        }
        if let Some((_, rts)) = aud.as_mut() {
            for rt in rts.iter_mut() {
                rt.pb = [[0.0; BATCH]; 2];
            }
        }
        let renderer = Renderer {
            underruns: &stats.underruns,
        };
        let mut off = 0usize;
        while off < BATCH {
            let rem = (BATCH - off) as u64;
            let (len, wraps) = match lp {
                Some((_, le)) if *pos <= le && le < *pos + rem => (le - *pos, true),
                _ => (rem, false),
            };
            let (a, b) = (*pos, *pos + len);
            if let Some((tracks, cursors)) = parts.as_mut() {
                emit_notes(map, sr, tracks, cursors, a, b, off, &mut em);
            }
            if b > a
                && let Some((tracks, rts)) = aud.as_mut()
            {
                for (t, rt) in tracks.iter().zip(rts.iter_mut()) {
                    renderer.segment(t, rt, a, b, off);
                }
            }
            // Beats are >= 480 frames apart, so a segment holds at most one onset.
            let mut beat = meters.beat_at_or_before(map.tick(a as f64, sr).floor() as i64);
            loop {
                let f = map.song_frame(beat.0, sr);
                if f >= b {
                    break;
                }
                if f >= a && *click_count < clicks.len() {
                    clicks[*click_count] = ((off + (f - a) as usize) as u16, beat.1);
                    *click_count += 1;
                }
                beat = meters.beat_after(beat.0);
            }
            if sync_cfg.enabled {
                clocks_in(map, sr, a, b, |f| {
                    sync.clock(frame + off as u64 + (f - a));
                });
            }
            *pos = b;
            off += len as usize;
            if wraps && let Some((ls, _)) = lp {
                wrap(map, sr, &mut parts, &mut em, off, ls);
                *pos = ls;
                *loop_count += 1;
                sync.wrapped(frame + off as u64, self.loop_start, sync_cfg);
                if time.loop_wrap.is_none() && parts.is_some() && off < BATCH {
                    time.loop_wrap = Some(LoopWrap {
                        offset: off as u16,
                        to_tick: map.tick(ls as f64, sr),
                    });
                }
            }
        }
        if let Some((tracks, rts)) = aud.as_mut() {
            publish_audio(arena, tracks, rts, *pos);
        }
        em.events.sort_unstable_by_key(|e| (e.offset, e.seq));
        time
    }

    /// Adds the metronome to interface outputs 1/2 (after the routing wrote them). With a
    /// `delay` line (return alignment on) the click is held back by the output latency, so it
    /// lands with the audio it was timed against.
    pub(crate) fn mix_metronome(&mut self, out: &mut [Batch], delay: Option<&mut DelayLine>) {
        let Some(line) = delay else {
            self.mix_click(out);
            return;
        };
        // The click goes through the line even while silent, so a click in flight at a stop
        // still sounds out.
        let mut click = [[0.0; BATCH]];
        self.mix_click(&mut click);
        let mut late = [0.0; BATCH];
        line.process(&click[0], &mut late);
        for ch in out.iter_mut().take(2) {
            for (o, x) in ch.iter_mut().zip(&late) {
                *o += *x;
            }
        }
    }

    /// Adds the click of this batch to the first two of `out`.
    fn mix_click(&mut self, out: &mut [Batch]) {
        if !self.counting && (!self.playing || !self.metronome) {
            self.click = None;
            return;
        }
        let mut next = 0;
        for i in 0..BATCH {
            while next < self.click_count && usize::from(self.clicks[next].0) == i {
                self.click = Some((0, self.clicks[next].1));
                next += 1;
            }
            let Some((n, accent)) = self.click else {
                continue;
            };
            let table = if accent { &self.accent } else { &self.beat };
            let s = self.gain * table[n];
            for ch in out.iter_mut().take(2) {
                ch[i] += s;
            }
            self.click = (n + 1 < table.len()).then_some((n + 1, accent));
        }
    }
}
