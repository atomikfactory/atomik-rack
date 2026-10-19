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
//!   for a held key (so locating into a note never sends its off). Stop and Locate while
//!   playing send `AllOff` to every track input and clear the sets; a loop wrap releases the
//!   held keys with explicit `Off`s; a snapshot swap transfers the set of an unchanged track
//!   (same device input, same content hash) and releases every other held key with `Off`s.
//! * **Merge order.** Song events are generated per segment in track order, then sorted by
//!   (offset, generation order). The engine merges them with the scheduled and live queues by
//!   frame; on equal frames: song, then scheduled, then live.
//! * **Automation.** At each playing batch start (after a pending wrap), every lane is
//!   evaluated at the exact fractional batch-start tick and applied with the clamping
//!   `set_param`; nothing is applied while stopped.
//! * **Metronome.** Beats are every `PPQ` ticks (`F(k * PPQ)`), accented every
//!   `beats_per_bar`. A click is a 30 ms decaying sine burst from a table computed when the
//!   engine is created (1760 Hz accent at full level, 1320 Hz at 0.6), added to interface
//!   outputs 1/2 times the gain, only while playing (stopping cuts it).
//! * **Tempo.** With a song loaded, the song's tempo map drives positions and `TimeInfo::tempo`
//!   (the tempo at the batch start); `set_tempo` only matters for song-less racks, where it
//!   drives the transport too (the tick is kept when it changes while playing).

use crate::arena::Arena;
use crate::audio::{AudioRt, CompiledAudio, Renderer};
use crate::graph::DeviceInstanceId;
use crate::seq::{
    CompiledTrack, SongCurve, SongPoint, SongSnapshot, TempoMap, TempoSegment, TrackCursor,
    TransportCommand, TransportState,
};
use crate::stream::{StreamRequest, StreamStats};
use atomik_core::{BATCH, Batch, LoopWrap, NoteKind, PPQ, Tempo};
use atomik_device_sdk::SocketId;
use rtrb::Producer;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering, fence};

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
}

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
            let s = TransportState {
                playing: flags & 1 != 0,
                tick: self.tick.load(Ordering::Relaxed) as i64,
                engine_frame: self.engine_frame.load(Ordering::Relaxed),
                loop_count: self.loop_count.load(Ordering::Relaxed),
                song_frame: self.song_frame.load(Ordering::Relaxed),
                loop_start: self.loop_start.load(Ordering::Relaxed) as i64,
                loop_end: self.loop_end.load(Ordering::Relaxed) as i64,
                loop_enabled: flags & 2 != 0,
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

    /// `AllOff` to every track input at offset 0.
    fn all_off(&mut self, tracks: &[CompiledTrack], cursors: &mut [TrackCursor]) {
        for (t, c) in tracks.iter().zip(cursors) {
            if self.push(0, t.device, t.socket, NoteKind::AllOff) {
                c.held = 0;
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

/// A loop wrap at batch offset `at`: releases every held key, then seeks to `ls`.
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
                // Song tracks only hold On and Off; the channel kinds of plugin notes never come
                // from a clip.
                NoteKind::AllOff
                | NoteKind::PitchBend { .. }
                | NoteKind::Cc { .. }
                | NoteKind::ChannelPressure { .. }
                | NoteKind::PolyPressure { .. }
                | NoteKind::NoteExpr { .. } => {}
            }
            c.next += 1;
        }
    }
}

/// The lane's value at a fractional tick: held before the first and after the last point,
/// linear or step in between (equal ticks: the last point wins, a jump).
pub(crate) fn lane_value(points: &[SongPoint], tick: f64) -> f32 {
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
    /// `(numerator, denominator)`: the song's beats per bar over 4; 4/4 without a song.
    pub(crate) time_signature: (u8, u8),
}

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
            requests,
            stats,
        }
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
        let old_map = old
            .as_deref()
            .map_or(TempoMap(&constant), SongSnapshot::map);
        let new_map = TempoMap(&new.tempo);
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
        seek(TempoMap(tempo), sr, tracks, cursors, self.pos);
        self.song = Some(new);
        old
    }

    /// Applies a transport command at this batch boundary.
    pub(crate) fn command(
        &mut self,
        command: TransportCommand,
        engine_tempo: Tempo,
        sr: f64,
        dropped: &AtomicU64,
    ) {
        let constant = [TempoSegment::constant(engine_tempo)];
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
                TempoMap(&s.tempo),
                Some((&s.tracks[..], &mut s.cursors[..])),
            ),
            None => (TempoMap(&constant), None),
        };
        let mut em = Emitter {
            events,
            seq,
            dropped,
        };
        match command {
            TransportCommand::Play => {
                if !*playing {
                    *playing = true;
                    if let Some((tracks, cursors)) = parts.as_mut() {
                        seek(map, sr, tracks, cursors, *pos);
                    }
                }
            }
            TransportCommand::Stop => {
                if *playing {
                    *playing = false;
                    self.click = None;
                    if let Some((tracks, cursors)) = parts.as_mut() {
                        em.all_off(tracks, cursors);
                    }
                }
            }
            TransportCommand::Locate(tick) => {
                if let Some((tracks, cursors)) = parts.as_mut() {
                    if *playing {
                        em.all_off(tracks, cursors);
                    }
                    *pos = map.song_frame(tick.0, sr);
                    seek(map, sr, tracks, cursors, *pos);
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
        let constant = [TempoSegment::constant(engine_tempo)];
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
            ..
        } = self;
        let (map, lanes, mut parts, beats_per_bar, mut aud) = match song.as_deref_mut() {
            Some(s) => (
                TempoMap(&s.tempo),
                &s.lanes[..],
                Some((&s.tracks[..], &mut s.cursors[..])),
                u64::from(s.beats_per_bar.max(1)),
                Some((&s.audio[..], &mut s.audio_rt[..])),
            ),
            None => (TempoMap(&constant), &[][..], None, 4, None),
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
        if *playing
            && let Some((ls, le)) = lp
            && *pos == le
        {
            wrap(map, sr, &mut parts, &mut em, 0, ls);
            *pos = ls;
            *loop_count += 1;
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
            time_signature: if parts.is_some() {
                (u8::try_from(beats_per_bar).unwrap_or(u8::MAX), 4)
            } else {
                (4, 4)
            },
        };
        *click_count = 0;
        if !*playing {
            return time;
        }
        for lane in lanes {
            arena.set_param(lane.device, lane.param, lane_value(&lane.points, tick0));
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
            let mut k = (map.tick(a as f64, sr) / PPQ as f64).floor().max(0.0) as i64;
            loop {
                let f = map.song_frame(k * PPQ, sr);
                if f >= b {
                    break;
                }
                if f >= a && *click_count < clicks.len() {
                    clicks[*click_count] = (
                        (off + (f - a) as usize) as u16,
                        (k as u64).is_multiple_of(beats_per_bar),
                    );
                    *click_count += 1;
                }
                k += 1;
            }
            *pos = b;
            off += len as usize;
            if wraps && let Some((ls, _)) = lp {
                wrap(map, sr, &mut parts, &mut em, off, ls);
                *pos = ls;
                *loop_count += 1;
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

    /// Adds the metronome to interface outputs 1/2 (after the routing wrote them).
    pub(crate) fn mix_metronome(&mut self, out: &mut [Batch]) {
        if !self.playing || !self.metronome {
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
