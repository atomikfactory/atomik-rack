//! The clip launcher on the audio thread: slots that play looped over the arrangement.
//!
//! # Model
//!
//! A *launch track* is a note track that has launcher slots. Each slot is compiled off the audio
//! thread into a [`CompiledTrack`] whose event ticks are *slot ticks* (`0..=length`: notes are
//! cut at the loop end). A track is either in the **Arrangement** state (it plays its arrangement
//! clips, as ever) or **Launched(slot)**: the slot plays and the track's arrangement events are
//! silent (their cursor still advances, so Back to Arrangement finds its place at once).
//!
//! A launch or a stop is a [`LaunchCommand`] that sits *queued* on its track until the next launch
//! quantize boundary of the song position. A boundary is a song tick, so it is exact through
//! tempo ramps and meter changes; its frame is the song's own tick-to-frame map. A slot that
//! starts at the song tick `S` plays its cycle `k` over the song ticks `S + k * length ..` (the
//! tick of an event is `S + k * length + event tick`), so the loop never drifts, whatever the
//! tempo does, and the playing position is a pure function of the song tick. A locate, a loop
//! wrap or a snapshot swap re-derives the cursor from it.
//!
//! # Transport rule
//!
//! Requests wait while the transport is stopped (the boundary is worked out in the first playing
//! batch, from the position then); the app starts the transport when the user launches with it
//! stopped (see `AppCore::launch`). A stop of the transport keeps the launched slots; play
//! resumes them where the song tick says they are. Notes that are on when a slot stops, is
//! replaced, or the transport stops get their note-offs; a slot does not chase notes that
//! span a locate (they start at their next note-on).
//!
//! Nothing here allocates: the state is fixed-size and lives in the snapshot, the queue
//! entries are small `Copy` values, and the events for the app go through a lock-free ring.

use crate::audio::{AudioRt, memory_regions};
use crate::graph::DeviceInstanceId;
use crate::player::Emitter;
use crate::seq::{CompiledTrack, SongNote, SongTrack, TrackCursor, compile_track};
use atomik_core::{BATCH, Batch, MeterMap, NoteKind, TempoMap};
use atomik_device_sdk::SocketId;
use rtrb::Producer;
use std::sync::atomic::{AtomicU32, Ordering};

/// The most launch tracks whose state is published for the UI.
pub const MAX_LAUNCH_TRACKS: usize = 128;
/// "No slot" in a published state.
pub const NO_SLOT: u16 = u16::MAX;
/// Published as the queued slot while the track is queued to go back to the arrangement.
pub const QUEUED_STOP: u16 = u16::MAX - 1;

/// Where a launch waits for: a grid of song ticks, or whole bars of the meter map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchGrid {
    /// A multiple of this many ticks (1 is the next tick).
    Ticks(i64),
    /// The first bar line of a group of this many bars, counted from bar 1.
    Bars(u32),
}

/// A request to the launcher (sent in order with the transport commands).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LaunchCommand {
    /// Play slot `slot` of the track on `device` from the next boundary of `grid`.
    Launch {
        /// The track's device.
        device: DeviceInstanceId,
        /// The slot (a scene row).
        slot: u16,
        /// Where to wait for.
        grid: LaunchGrid,
    },
    /// Stop the track's slot at the next boundary: the track plays its arrangement again.
    Stop {
        /// The track's device.
        device: DeviceInstanceId,
        /// Where to wait for.
        grid: LaunchGrid,
    },
    /// Every launched track goes back to its arrangement at the next boundary.
    StopAll {
        /// Where to wait for.
        grid: LaunchGrid,
    },
}

/// One slot of one track as the engine gets it: notes in slot ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchSlotInput {
    /// The loop length in ticks (> 0).
    pub length: i64,
    /// Notes in slot ticks (a note past `length` is cut there).
    pub notes: Vec<SongNote>,
    /// Pitch bend, aftertouch and controller lanes in slot ticks.
    pub controllers: Vec<crate::SongController>,
    /// Per-note expression in slot ticks.
    pub expression: Vec<crate::SongExpression>,
    /// The audio of an audio track's slot (and no notes).
    pub audio: Option<LaunchAudioInput>,
}

/// The audio of one slot: regions in slot frames (frame 0 is the slot start), the gain
/// envelopes they use and the assets they read (memory assets; a streamed one stays silent).
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchAudioInput {
    /// Regions, as [`crate::SongAudioRegion`] with song frames counted from the slot start.
    pub regions: Vec<crate::SongAudioRegion>,
    /// Gain envelopes ([`crate::SongAudioTrack::envelopes`]).
    pub envelopes: Vec<Vec<(u64, f32)>>,
    /// The assets (`AssetId(n)` is `assets[n]`).
    pub assets: Vec<crate::SongAsset>,
}

/// The slots of one track.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchTrackInput {
    /// Target device.
    pub device: DeviceInstanceId,
    /// Target Notes input socket.
    pub notes_socket: SocketId,
    /// Slots by scene row.
    pub slots: Vec<Option<LaunchSlotInput>>,
}

/// A compiled slot.
#[derive(Clone)]
pub(crate) struct CompiledSlot {
    pub(crate) length: i64,
    pub(crate) track: CompiledTrack,
    /// The audio of an audio slot.
    pub(crate) audio: Option<std::sync::Arc<crate::audio::LaneData>>,
}

/// A compiled launch track.
#[derive(Clone)]
pub(crate) struct LaunchTrack {
    pub(crate) device: DeviceInstanceId,
    pub(crate) socket: SocketId,
    /// The arrangement track of the same device and socket, if the song has one.
    pub(crate) link: Option<usize>,
    /// An audio track: its slots play audio, and `audio_link` is its arrangement.
    pub(crate) is_audio: bool,
    /// The arrangement audio track of the same device, if the song has one.
    pub(crate) audio_link: Option<usize>,
    pub(crate) slots: Vec<Option<CompiledSlot>>,
}

/// A request waiting for its boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Queued {
    /// The slot to play, or `None` to go back to the arrangement.
    target: Option<u16>,
    grid: LaunchGrid,
    /// The boundary tick, once worked out (in the first playing batch).
    at: Option<i64>,
}

/// The audio thread's state of one launch track.
#[derive(Clone)]
pub(crate) struct LaunchRt {
    /// The playing slot, `None` in the arrangement state.
    pub(crate) slot: Option<u16>,
    /// The song tick the slot started at.
    start: i64,
    /// The cycle of the slot the cursor is in.
    cycle: i64,
    /// The next event of the slot.
    next: usize,
    /// Bit `k` set while the slot holds key `k` on.
    held: u128,
    pub(crate) queued: Option<Queued>,
    /// Audio slot: the song frame it started at and the frames of one cycle.
    start_frame: u64,
    cycle_frames: u64,
    /// Audio slot: what it renders in this batch.
    pb: [Batch; 2],
}

impl Default for LaunchRt {
    fn default() -> Self {
        Self {
            slot: None,
            start: 0,
            cycle: 0,
            next: 0,
            held: 0,
            queued: None,
            start_frame: 0,
            cycle_frames: 1,
            pb: [[0.0; BATCH]; 2],
        }
    }
}

/// Something the launcher did, for the app (it records launches into the arrangement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchEvent {
    /// The track's device.
    pub device: DeviceInstanceId,
    /// The slot that started, or `None` when the track went back to its arrangement.
    pub slot: Option<u16>,
    /// The song tick it happened at.
    pub tick: i64,
}

/// What the UI sees of one launch track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaunchState {
    /// The track's device.
    pub device: DeviceInstanceId,
    /// The playing slot.
    pub playing: Option<u16>,
    /// The slot waiting for its boundary.
    pub queued: Option<u16>,
    /// The track waits to go back to the arrangement.
    pub queued_stop: bool,
    /// Where the playing slot is in its loop, `0.0..1.0`.
    pub phase: f32,
}

#[derive(Debug, Default)]
struct PubEntry {
    device: AtomicU32,
    /// `playing | queued << 16`.
    slots: AtomicU32,
    phase: AtomicU32,
}

/// The launch states the audio thread publishes once per batch (a few atomics per track; a
/// read that races a write sees a slightly old track, which only shifts a display by a batch).
#[derive(Debug)]
pub struct LaunchCell {
    count: AtomicU32,
    entries: Box<[PubEntry]>,
}

impl Default for LaunchCell {
    fn default() -> Self {
        Self {
            count: AtomicU32::new(0),
            entries: (0..MAX_LAUNCH_TRACKS)
                .map(|_| PubEntry::default())
                .collect(),
        }
    }
}

impl LaunchCell {
    /// The published states, one per launch track, in the order of the song's launch list.
    pub fn read(&self, out: &mut Vec<LaunchState>) {
        out.clear();
        let n = (self.count.load(Ordering::Acquire) as usize).min(MAX_LAUNCH_TRACKS);
        for e in &self.entries[..n] {
            let slots = e.slots.load(Ordering::Relaxed);
            let (playing, queued) = ((slots & 0xFFFF) as u16, (slots >> 16) as u16);
            out.push(LaunchState {
                device: DeviceInstanceId(e.device.load(Ordering::Relaxed)),
                playing: (playing != NO_SLOT).then_some(playing),
                queued: (queued != NO_SLOT && queued != QUEUED_STOP).then_some(queued),
                queued_stop: queued == QUEUED_STOP,
                phase: f32::from_bits(e.phase.load(Ordering::Relaxed)),
            });
        }
    }

    fn publish(
        &self,
        tracks: &[LaunchTrack],
        rt: &[LaunchRt],
        map: TempoMap<'_>,
        sr: f64,
        pos: u64,
    ) {
        let tick = map.tick(pos as f64, sr).round() as i64;
        for (i, (t, r)) in tracks.iter().zip(rt).enumerate().take(MAX_LAUNCH_TRACKS) {
            let e = &self.entries[i];
            let playing = r.slot.unwrap_or(NO_SLOT);
            let queued = match r.queued {
                Some(Queued {
                    target: Some(s), ..
                }) => s,
                Some(_) => QUEUED_STOP,
                None => NO_SLOT,
            };
            let phase = match r.slot.and_then(|s| t.slots.get(usize::from(s))?.as_ref()) {
                Some(slot) => {
                    let into = (tick - r.start).rem_euclid(slot.length.max(1));
                    into as f32 / slot.length.max(1) as f32
                }
                None => 0.0,
            };
            e.device.store(t.device.0, Ordering::Relaxed);
            e.slots.store(
                u32::from(playing) | u32::from(queued) << 16,
                Ordering::Relaxed,
            );
            e.phase.store(phase.to_bits(), Ordering::Relaxed);
        }
        self.count.store(
            tracks.len().min(MAX_LAUNCH_TRACKS) as u32,
            Ordering::Release,
        );
    }
}

/// Compiles the launch tracks of a song (off the audio thread). `arrangement` and `audio` are the
/// compiled arrangement tracks, to link each launch track to its own.
pub(crate) fn compile_launch(
    input: &[LaunchTrackInput],
    arrangement: &[CompiledTrack],
    audio: &[crate::audio::CompiledAudio],
) -> Result<Vec<LaunchTrack>, crate::SongError> {
    let mut out: Vec<LaunchTrack> = Vec::with_capacity(input.len());
    for t in input {
        if out
            .iter()
            .any(|o| o.device == t.device && o.socket == t.notes_socket)
        {
            return Err(crate::SongError::DuplicateTrack(t.device, t.notes_socket));
        }
        let mut slots = Vec::with_capacity(t.slots.len());
        let mut is_audio = false;
        for s in &t.slots {
            slots.push(match s {
                None => None,
                Some(s) => {
                    if s.length <= 0 {
                        return Err(crate::SongError::BadEvent(format!(
                            "slot length {}",
                            s.length
                        )));
                    }
                    // Notes are cut at the loop end; one that starts past it never plays.
                    let notes = s
                        .notes
                        .iter()
                        .filter(|n| n.tick.0 < s.length)
                        .map(|n| SongNote {
                            length: n.length.min(s.length - n.tick.0),
                            ..*n
                        })
                        .collect();
                    let track = SongTrack {
                        device: t.device,
                        notes_socket: t.notes_socket,
                        notes,
                        automation: Vec::new(),
                        chase: false,
                        controllers: s.controllers.clone(),
                        expression: s.expression.clone(),
                    };
                    let lane = match &s.audio {
                        Some(a) => {
                            is_audio = true;
                            let compiled = crate::audio::compile_audio(
                                &[crate::SongAudioTrack {
                                    device: t.device,
                                    regions: a.regions.clone(),
                                    envelopes: a.envelopes.clone(),
                                    warp_maps: Vec::new(),
                                }],
                                &a.assets,
                            )?;
                            compiled.into_iter().next().map(|c| c.data)
                        }
                        None => None,
                    };
                    Some(CompiledSlot {
                        length: s.length,
                        track: compile_track(&track)?,
                        audio: lane,
                    })
                }
            });
        }
        let link = arrangement
            .iter()
            .position(|a| a.device == t.device && a.socket == t.notes_socket);
        let audio_link = audio.iter().position(|a| a.device == t.device);
        out.push(LaunchTrack {
            device: t.device,
            socket: t.notes_socket,
            link: if is_audio { None } else { link },
            is_audio,
            audio_link: if is_audio { audio_link } else { None },
            slots,
        });
    }
    Ok(out)
}

fn ceil_div(a: i64, n: i64) -> i64 {
    a.div_euclid(n) + i64::from(a.rem_euclid(n) != 0)
}

/// The first boundary of `grid` at or after `tick`.
pub(crate) fn boundary(grid: LaunchGrid, tick: i64, meters: MeterMap<'_>) -> i64 {
    match grid {
        LaunchGrid::Ticks(n) => ceil_div(tick, n.max(1)) * n.max(1),
        LaunchGrid::Bars(n) => {
            let n = n.max(1);
            let mut bar = meters.bar_at(tick);
            if meters.bar_start(bar) < tick {
                bar += 1;
            }
            while !(bar - 1).is_multiple_of(n) {
                bar += 1;
            }
            meters.bar_start(bar)
        }
    }
}

/// Queues a request (a launch replaces what was queued on its track).
pub(crate) fn command(tracks: &[LaunchTrack], rt: &mut [LaunchRt], command: LaunchCommand) {
    match command {
        LaunchCommand::Launch { device, slot, grid } => {
            for (t, r) in tracks.iter().zip(rt) {
                if t.device == device && t.slots.get(usize::from(slot)).is_some_and(Option::is_some)
                {
                    r.queued = Some(Queued {
                        target: Some(slot),
                        grid,
                        at: None,
                    });
                }
            }
        }
        LaunchCommand::Stop { device, grid } => {
            for (t, r) in tracks.iter().zip(rt) {
                if t.device == device {
                    r.queued = (r.slot.is_some()).then_some(Queued {
                        target: None,
                        grid,
                        at: None,
                    });
                }
            }
        }
        LaunchCommand::StopAll { grid } => {
            for r in rt {
                r.queued = r.slot.is_some().then_some(Queued {
                    target: None,
                    grid,
                    at: None,
                });
            }
        }
    }
}

/// Works out the boundary of every queued request that has none, from song frame `pos`.
pub(crate) fn resolve(
    rt: &mut [LaunchRt],
    map: TempoMap<'_>,
    meters: MeterMap<'_>,
    sr: f64,
    pos: u64,
) {
    for r in rt {
        if let Some(q) = r.queued.as_mut()
            && q.at.is_none()
        {
            let tick = map.tick(pos as f64, sr).round() as i64;
            q.at = Some(boundary(q.grid, tick, meters));
        }
    }
}

/// Forgets the boundaries (a stop or a locate moved the position): they are worked out again
/// from the next playing batch.
pub(crate) fn unresolve(rt: &mut [LaunchRt]) {
    for r in rt {
        if let Some(q) = r.queued.as_mut() {
            q.at = None;
        }
    }
}

/// Points every playing slot's cursor at its first event at or after song frame `pos`.
pub(crate) fn seek(
    map: TempoMap<'_>,
    sr: f64,
    tracks: &[LaunchTrack],
    rt: &mut [LaunchRt],
    pos: u64,
) {
    for (t, r) in tracks.iter().zip(rt) {
        let Some(slot) = r.slot.and_then(|s| t.slots.get(usize::from(s))?.as_ref()) else {
            continue;
        };
        let events = &slot.track.events;
        if events.is_empty() {
            r.next = 0;
            continue;
        }
        let tick = map.tick(pos as f64, sr).floor() as i64;
        // One cycle early, then forward: the first event at or after `pos`.
        r.cycle = (tick - r.start).div_euclid(slot.length) - 1;
        loop {
            let base = r.start + r.cycle * slot.length;
            r.next = events.partition_point(|e| map.song_frame(base + e.tick, sr) < pos);
            if r.next < events.len() {
                break;
            }
            r.cycle += 1;
        }
    }
}

/// Releases what the slots hold on (explicit `Off`s at `offset`) and clears the held sets.
pub(crate) fn release(
    tracks: &[LaunchTrack],
    rt: &mut [LaunchRt],
    em: &mut Emitter<'_>,
    offset: usize,
) {
    for (t, r) in tracks.iter().zip(rt) {
        em.release_track(t.device, t.socket, &mut r.held, offset);
    }
}

/// The transport stopped or located while playing: the devices got `AllOff`, so the slots hold
/// nothing; queued boundaries are worked out again.
pub(crate) fn all_off(rt: &mut [LaunchRt]) {
    for r in rt.iter_mut() {
        r.held = 0;
    }
    unresolve(rt);
}

/// Marks the arrangement cursors of launched tracks silent (after a seek or a swap).
pub(crate) fn mark_silent(
    tracks: &[LaunchTrack],
    rt: &[LaunchRt],
    cursors: &mut [TrackCursor],
    audio: &mut [AudioRt],
) {
    for (t, r) in tracks.iter().zip(rt) {
        if let Some(c) = t.link.and_then(|l| cursors.get_mut(l)) {
            c.silent = r.slot.is_some();
        }
        if let Some(a) = t.audio_link.and_then(|l| audio.get_mut(l)) {
            a.silent = r.slot.is_some();
        }
    }
}

/// A launch event for the app; dropped when the ring is full.
fn note_event(
    events: &mut Producer<LaunchEvent>,
    device: DeviceInstanceId,
    slot: Option<u16>,
    tick: i64,
) {
    let _ = events.push(LaunchEvent { device, slot, tick });
}

/// Emits the events of song frames `[a, b)` at batch offsets from `base`: the arrangement of the
/// tracks that are not launched, the slots that play, and the launches and stops whose boundary
/// falls inside, each at its own frame.
#[allow(clippy::too_many_arguments)] // Justification: one hot RT helper; a struct would only rename them.
pub(crate) fn emit(
    map: TempoMap<'_>,
    sr: f64,
    arrangement: (&[CompiledTrack], &mut [TrackCursor]),
    launch: (&[LaunchTrack], &mut [LaunchRt]),
    audio: &mut [AudioRt],
    (a, b, base): (u64, u64, usize),
    em: &mut Emitter<'_>,
    events: &mut Producer<LaunchEvent>,
) {
    let (tracks, cursors) = arrangement;
    let (lt, lr) = launch;
    let mut from = a;
    loop {
        // The earliest queued boundary inside the rest of the segment.
        let mut at: Option<u64> = None;
        for r in lr.iter() {
            if let Some(Queued { at: Some(t), .. }) = r.queued {
                let f = map.song_frame(t, sr).max(from);
                if f < b && at.is_none_or(|x| f < x) {
                    at = Some(f);
                }
            }
        }
        let end = at.unwrap_or(b);
        let offset = base + (from - a) as usize;
        crate::player::emit_notes(map, sr, tracks, cursors, from, end, offset, em);
        for (t, r) in lt.iter().zip(lr.iter_mut()) {
            emit_slot(map, sr, t, r, (from, end, offset), em);
            render_audio_slot(t, r, (from, end, offset));
        }
        let Some(end) = at else { break };
        let offset = base + (end - a) as usize;
        for i in 0..lt.len() {
            let Some(q) = lr[i].queued else { continue };
            let Some(t) = q.at else { continue };
            if map.song_frame(t, sr).max(from) != end {
                continue;
            }
            switch(
                map,
                sr,
                (tracks, &mut *cursors),
                (&lt[i], &mut lr[i]),
                (q, end),
                offset,
                em,
                &mut *audio,
            );
            note_event(events, lt[i].device, lr[i].slot, t);
        }
        from = end;
    }
}

/// A launch or a stop reaches its boundary at song frame `frame` (batch offset `offset`).
#[allow(clippy::too_many_arguments)] // Justification: one RT helper; a struct would only rename them.
fn switch(
    map: TempoMap<'_>,
    sr: f64,
    (tracks, cursors): (&[CompiledTrack], &mut [TrackCursor]),
    (t, r): (&LaunchTrack, &mut LaunchRt),
    (q, frame): (Queued, u64),
    offset: usize,
    em: &mut Emitter<'_>,
    audio: &mut [AudioRt],
) {
    let tick = q.at.unwrap_or(0);
    // What sounds on the track now ends here.
    match (r.slot, t.link) {
        (Some(_), _) => em.release_track(t.device, t.socket, &mut r.held, offset),
        (None, Some(l)) => {
            em.release_track(t.device, t.socket, &mut cursors[l].held, offset);
            em.reset_controllers(&tracks[l], offset);
        }
        (None, None) => {}
    }
    r.queued = None;
    match q
        .target
        .filter(|s| t.slots.get(usize::from(*s)).is_some_and(Option::is_some))
    {
        Some(slot) => {
            r.slot = Some(slot);
            r.start = tick;
            r.cycle = 0;
            r.next = 0;
            r.held = 0;
            if let Some(c) = t.link.and_then(|l| cursors.get_mut(l)) {
                c.silent = true;
            }
            if t.is_audio {
                // An audio slot loops over `cycle_frames` song frames from where it starts; the
                // length is the tempo's at the start (the audio itself plays unwarped).
                let len = t.slots[usize::from(slot)].as_ref().map_or(1, |s| s.length);
                r.start_frame = map.song_frame(tick, sr);
                r.cycle_frames = (map.song_frame(tick + len, sr) - r.start_frame).max(1);
            }
            if let Some(a) = t.audio_link.and_then(|l| audio.get_mut(l)) {
                a.silent = true;
            }
        }
        None => {
            // The arrangement audio of this track comes back with the next batch (`mix_audio`).
            if r.slot.take().is_some()
                && let Some(l) = t.link
            {
                cursors[l].silent = false;
                let (tr, cu) = (&tracks[l..=l], &mut cursors[l..=l]);
                crate::player::seek(map, sr, tr, cu, frame);
                crate::player::chase(map, sr, tr, cu, frame, offset, em);
            }
            r.slot = None;
        }
    }
}

/// Emits the playing slot's events of song frames `[a, b)`.
fn emit_slot(
    map: TempoMap<'_>,
    sr: f64,
    t: &LaunchTrack,
    r: &mut LaunchRt,
    (a, b, off): (u64, u64, usize),
    em: &mut Emitter<'_>,
) {
    let Some(slot) = r.slot.and_then(|s| t.slots.get(usize::from(s))?.as_ref()) else {
        return;
    };
    let events = &slot.track.events;
    if events.is_empty() || b <= a {
        return;
    }
    loop {
        let Some(ev) = events.get(r.next) else {
            r.next = 0;
            r.cycle += 1;
            continue;
        };
        let tick = r.start + r.cycle * slot.length + ev.tick;
        let f = map.song_frame(tick, sr);
        if f >= b {
            break;
        }
        let offset = off + f.saturating_sub(a) as usize;
        match ev.kind {
            NoteKind::On { key, .. } => {
                if em.push(offset, t.device, t.socket, ev.kind) {
                    r.held |= 1u128 << key;
                }
            }
            NoteKind::Off { key } => {
                let bit = 1u128 << key;
                if r.held & bit != 0 && em.push(offset, t.device, t.socket, ev.kind) {
                    r.held &= !bit;
                }
            }
            NoteKind::PitchBend { .. }
            | NoteKind::Cc { .. }
            | NoteKind::ChannelPressure { .. }
            | NoteKind::PolyPressure { .. } => {
                em.push(offset, t.device, t.socket, ev.kind);
            }
            NoteKind::NoteExpr { key, .. } => {
                if r.held & (1u128 << key) != 0 {
                    em.push(offset, t.device, t.socket, ev.kind);
                }
            }
            NoteKind::AllOff => {}
        }
        r.next += 1;
    }
}

/// Publishes the launch states (once per batch).
pub(crate) fn publish(
    cell: &LaunchCell,
    tracks: &[LaunchTrack],
    rt: &[LaunchRt],
    map: TempoMap<'_>,
    sr: f64,
    pos: u64,
) {
    cell.publish(tracks, rt, map, sr, pos);
}

/// Carries the launch state of `old` over to `new` (a snapshot swap): by device and socket, a
/// playing slot keeps playing if the new song still has it; what its notes hold on is kept when
/// the slot's content is unchanged and released otherwise. Queued requests move over.
pub(crate) fn carry(
    old: (&[LaunchTrack], &[LaunchRt]),
    new: (&[LaunchTrack], &mut [LaunchRt]),
    em: &mut Emitter<'_>,
) {
    let (ot, or) = old;
    let (nt, nr) = new;
    for (o, r) in ot.iter().zip(or) {
        let Some(j) = nt
            .iter()
            .position(|n| n.device == o.device && n.socket == o.socket)
        else {
            let mut held = r.held;
            em.release_track(o.device, o.socket, &mut held, 0);
            continue;
        };
        let n = &nt[j];
        let mut held = r.held;
        let same = |slot: u16| {
            let was = o.slots.get(usize::from(slot)).and_then(Option::as_ref);
            let now = n.slots.get(usize::from(slot)).and_then(Option::as_ref);
            matches!((was, now), (Some(w), Some(c)) if w.track.hash == c.track.hash)
        };
        let exists = |slot: u16| n.slots.get(usize::from(slot)).is_some_and(Option::is_some);
        let new_rt = &mut nr[j];
        if let Some(q) = r.queued
            && q.target.is_none_or(exists)
        {
            new_rt.queued = Some(q);
        }
        match r.slot {
            Some(s) if exists(s) => {
                new_rt.slot = Some(s);
                new_rt.start = r.start;
                new_rt.cycle = r.cycle;
                new_rt.next = r.next;
                new_rt.start_frame = r.start_frame;
                new_rt.cycle_frames = r.cycle_frames;
                if same(s) {
                    new_rt.held = held;
                } else {
                    em.release_track(o.device, o.socket, &mut held, 0);
                }
            }
            Some(_) => em.release_track(o.device, o.socket, &mut held, 0),
            None => {}
        }
    }
}

/// A loop wrap at batch offset `at`: the slots release what they hold and find their place at
/// `ls` (a slot's place is a function of the song tick, so it stays on the song's grid).
pub(crate) fn wrap(
    map: TempoMap<'_>,
    sr: f64,
    (tracks, rt): (&[LaunchTrack], &mut [LaunchRt]),
    em: &mut Emitter<'_>,
    at: usize,
    ls: u64,
) {
    release(tracks, rt, em, at);
    seek(map, sr, tracks, rt, ls);
}

/// Clears what the audio slots render, at the start of a playing batch.
pub(crate) fn clear_audio(rt: &mut [LaunchRt]) {
    for r in rt {
        r.pb = [[0.0; BATCH]; 2];
    }
}

/// Renders the playing audio slot over song frames `[a, b)` (batch offset `off` for `a`): the
/// slot's regions play from the slot's start frame and loop every `cycle_frames`.
fn render_audio_slot(t: &LaunchTrack, r: &mut LaunchRt, (a, b, off): (u64, u64, usize)) {
    let Some(data) = r
        .slot
        .and_then(|s| t.slots.get(usize::from(s))?.as_ref())
        .and_then(|s| s.audio.as_ref())
    else {
        return;
    };
    let cycle = r.cycle_frames.max(1) as i64;
    let start = r.start_frame as i64;
    let mut p = a as i64;
    let end = b as i64;
    while p < end {
        // The cycle `p` is in (a position before the start counts backwards on the same grid).
        let k = (p - start).div_euclid(cycle);
        let base = start + k * cycle;
        let q = end.min(base + cycle);
        let (la, lb) = ((p - base) as u64, (q - base) as u64);
        memory_regions(data, &mut r.pb, la, lb, off + (p - a as i64) as usize, la);
        p = q;
    }
}

/// Hands what the audio slots rendered to their tracks: added to the arrangement's own buffer
/// of a track that has one (silent while the slot plays), set as the track's playback otherwise.
/// Call before the arrangement's buffers are published.
pub(crate) fn mix_audio(
    arena: &mut crate::arena::Arena,
    tracks: &[LaunchTrack],
    rt: &[LaunchRt],
    audio: &mut [AudioRt],
) {
    for (t, r) in tracks.iter().zip(rt) {
        if !t.is_audio {
            continue;
        }
        if r.slot.is_none()
            && let Some(a) = t.audio_link.and_then(|l| audio.get_mut(l))
        {
            // The slot stopped in this batch: the arrangement plays again from the next.
            a.silent = false;
        }
        match t.audio_link.and_then(|l| audio.get_mut(l)) {
            Some(a) => {
                for (dst, src) in a.pb.iter_mut().zip(&r.pb) {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d += *s;
                    }
                }
            }
            None => arena.set_playback(t.device, &r.pb),
        }
    }
}
