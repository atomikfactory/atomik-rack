//! Song playback: an immutable [`SongSnapshot`]
//! compiled off the RT thread, played by an RT sequencer inside the engine following a
//! transport.
//!
//! - [`SongInput`] is engine-native song data (device ids, param ids, ticks). The app builds
//!   it from the model; the engine never depends on the model.
//! - [`compile_song`] (non-RT) produces a [`SongSnapshot`]: per-track event lists sorted and
//!   pre-resolved so the RT player only walks indices (no allocation, no sorting on RT).
//! - Snapshots and transport commands travel through the ordered control queue.
//!   A retired snapshot is handed back and dropped by `collect_garbage`.
//! - Notes are sample-accurate; automation is applied once per 64-frame batch.
//! - Transport state is published lock-free for the UI and for recording ([`TransportState`]).
//! - Tempo: while a song is loaded its tempo map (steps and linear ramps, `atomik_core::TempoMap`)
//!   drives the transport and `TimeInfo::tempo`; `EngineHandle::set_tempo` only matters for
//!   song-less racks. The meter map gives `TimeInfo::time_signature` and the metronome's beats.
//! - Recording: [`SongSnapshot::position_at`] maps a live event's engine frame to the exact
//!   song frame and tick, replaying loop wraps from a published [`TransportState`].
//!
//! The RT player lives in `player.rs` (design, merge order and determinism notes there).
//!
//! Public signatures are a stable contract: changes need review.

use crate::controllers::{CtlLane, SongController, expand, validate};
use crate::graph::DeviceInstanceId;
use atomik_core::{
    MapError, MeterEvent, MeterMap, MeterSegment, NoteKind, ParamId, Tempo, TempoMap, TempoPoint,
    TempoSegment, Ticks, compile_meter, compile_tempo,
};
use atomik_device_sdk::SocketId;

/// One note event of a track, in song ticks (already flattened from clips, loops, offsets
/// and mutes by the caller or by [`compile_song`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SongNote {
    /// Song tick of note-on.
    pub tick: Ticks,
    /// Length in ticks (> 0).
    pub length: i64,
    /// MIDI key.
    pub key: u8,
    /// Velocity in (0, 1].
    pub velocity: f32,
}

/// Interpolation from one automation point to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SongCurve {
    /// Linear ramp to the next point.
    Linear,
    /// Hold until the next point.
    Step,
}

/// One automation point in song ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SongPoint {
    /// Song tick.
    pub tick: Ticks,
    /// Plain parameter value.
    pub value: f32,
    /// Segment shape to the next point.
    pub curve: SongCurve,
}

/// Automation of one parameter of a track's device.
#[derive(Debug, Clone, PartialEq)]
pub struct SongAutomation {
    /// Parameter.
    pub param: ParamId,
    /// Points sorted by tick (no automation outside the first/last point's range: before the
    /// first point and after the last point the value holds).
    pub points: Vec<SongPoint>,
}

/// One track: notes into a device's Notes input plus parameter automation.
#[derive(Debug, Clone, PartialEq)]
pub struct SongTrack {
    /// Target device.
    pub device: DeviceInstanceId,
    /// Target Notes input socket.
    pub notes_socket: SocketId,
    /// Notes (any order; `compile_song` sorts).
    pub notes: Vec<SongNote>,
    /// Automation lanes.
    pub automation: Vec<SongAutomation>,
    /// Note chase: when playback starts or jumps (play, locate while playing, loop wrap) at a
    /// position inside a note (`start < position < end`), the note sounds from there, with its
    /// own velocity, and its regular off releases it. `false` plays only notes that start at or
    /// after the position (a note spanning it stays silent).
    pub chase: bool,
    /// Pitch bend, aftertouch and controller lanes, sent to the same input next to the notes
    /// (see [`crate::SongController`]). At most one lane per kind. Unlike notes they are always
    /// chased: a seek restores each controller's value at the position.
    pub controllers: Vec<SongController>,
}

/// A tempo change at a tick: a step, or with `ramp` a linear ramp (in BPM, over ticks) to the
/// next event's tempo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SongTempo {
    /// Tick where it takes effect (the first must be at tick 0).
    pub tick: Ticks,
    /// Tempo.
    pub tempo: Tempo,
    /// Ramps to the next event's tempo instead of holding.
    pub ramp: bool,
}

/// Engine-native song input (built by the app from the model; muted/soloed tracks already
/// filtered by the caller).
#[derive(Debug, Clone, PartialEq)]
pub struct SongInput {
    /// Tracks.
    pub tracks: Vec<SongTrack>,
    /// Tempo map (sorted, first at tick 0).
    pub tempo: Vec<SongTempo>,
    /// Meter map (bars strictly increasing, first at bar 1): the time signature devices see and
    /// the metronome's beats, accented on the first beat of each bar.
    pub meter: Vec<MeterEvent>,
    /// Audio tracks: the clips of `atomik.audio-track` devices.
    pub audio: Vec<crate::SongAudioTrack>,
    /// The audio assets; `AssetId(n)` is `assets[n]`.
    pub assets: Vec<crate::SongAsset>,
}

/// Song compilation errors.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SongError {
    /// The tempo map is empty, does not start at tick 0, is not strictly increasing or holds a
    /// tempo outside 20..=999 BPM.
    #[error("tempo map must start at tick 0")]
    BadTempoMap,
    /// The meter map is empty, does not start at bar 1, is not strictly increasing or holds an
    /// unsupported time signature.
    #[error("invalid meter map: {0}")]
    BadMeterMap(MapError),
    /// A note or point has a negative tick or non-positive length.
    #[error("invalid event: {0}")]
    BadEvent(String),
    /// Two tracks target the same device Notes input.
    #[error("two tracks target device {0:?} socket {1:?}")]
    DuplicateTrack(DeviceInstanceId, SocketId),
    /// An audio region names an asset that is not in `SongInput::assets`.
    #[error("audio region uses unknown asset {0:?}")]
    UnknownAsset(crate::AssetId),
    /// Two audio tracks target the same device.
    #[error("two audio tracks target device {0:?}")]
    DuplicateAudioTrack(DeviceInstanceId),
    /// An asset's data does not match its channel count and frames.
    #[error("invalid asset: {0}")]
    BadAsset(String),
}

/// Immutable compiled song (owns its pre-sorted event arrays). Built off the RT thread.
///
/// Cloning is cheap compared to compiling; the app may keep a clone (or compile a tempo-only
/// song) for tick <-> frame conversions while the engine owns the submitted one.
#[derive(Clone)]
pub struct SongSnapshot {
    /// Tempo segments (first at tick 0, ticks strictly increasing).
    pub(crate) tempo: Vec<TempoSegment>,
    /// Meter segments (first at bar 1): the signature and the metronome's beats.
    pub(crate) meter: Vec<MeterSegment>,
    /// Tracks in input order.
    pub(crate) tracks: Vec<CompiledTrack>,
    /// `(device, socket, track index)` sorted, for matching tracks across swaps.
    pub(crate) by_key: Vec<(DeviceInstanceId, SocketId, usize)>,
    /// Automation lanes in input order (track order, then lane order).
    pub(crate) lanes: Vec<CompiledLane>,
    /// RT scratch, one per track: playback cursor and held keys. Only the engine that owns
    /// the snapshot touches it; the compiled content above never changes.
    pub(crate) cursors: Vec<TrackCursor>,
    /// Compiled audio tracks.
    pub(crate) audio: Vec<crate::audio::CompiledAudio>,
    /// RT scratch per audio track: the playback pair and the stream lane.
    pub(crate) audio_rt: Vec<crate::audio::AudioRt>,
}

/// One flattened note event of a track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SeqEvent {
    pub(crate) tick: i64,
    /// `On`, `Off` or a controller event (never `AllOff`).
    pub(crate) kind: NoteKind,
}

/// A track's flattened events, sorted by (tick, offs before ons, key).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CompiledTrack {
    pub(crate) device: DeviceInstanceId,
    pub(crate) socket: SocketId,
    pub(crate) events: Vec<SeqEvent>,
    /// The controller lanes (expanded), for the chase of controller values on a seek.
    pub(crate) ctl: Vec<CtlLane>,
    /// Notes sorted by start tick (per-key overlaps resolved), for chase and for the keys a
    /// swapped track still covers.
    pub(crate) notes: Vec<ChaseNote>,
    /// `max_end[i]` = the greatest end tick among `notes[..=i]` (lets a backwards walk from a
    /// position stop as soon as no earlier note can still be sounding).
    pub(crate) max_end: Vec<i64>,
    /// Whether a seek chases the notes spanning the position ([`SongTrack::chase`]).
    pub(crate) chase: bool,
    /// Content hash (device, socket, chase flag, events): equal hashes mean an unchanged track
    /// on swap.
    pub(crate) hash: u64,
}

/// One note of a compiled track, in ticks (frames depend on the tempo map and sample rate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChaseNote {
    pub(crate) start: i64,
    pub(crate) end: i64,
    pub(crate) key: u8,
    pub(crate) velocity: f32,
}

/// One automated parameter, points sorted by tick (stable).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CompiledLane {
    pub(crate) device: DeviceInstanceId,
    pub(crate) param: ParamId,
    pub(crate) points: Vec<SongPoint>,
}

/// Per-track RT state: the next event to emit and the keys the player holds on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TrackCursor {
    pub(crate) next: usize,
    /// Bit `k` set while key `k` is held by the player (128 keys).
    pub(crate) held: u128,
}

/// Where a live event at an engine frame falls in the song ([`SongSnapshot::position_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SongPosition {
    /// Integer song frame (the song timeline the player renders, tick 0 = frame 0).
    pub song_frame: u64,
    /// `frame_to_tick(song_frame)` (nearest tick).
    pub tick: Ticks,
    /// Loop wraps up to that frame (same counter as [`TransportState::loop_count`]).
    pub loop_count: u64,
}

impl std::fmt::Debug for SongSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SongSnapshot").finish_non_exhaustive()
    }
}

/// Compiles song input into a snapshot (non-RT; may allocate).
///
/// Validates the input ([`SongError`]), builds the tempo segment table, flattens each track's
/// notes into on/off events sorted by (tick, offs before ons, key) — overlapping notes of one
/// key are cut where the next one starts, and a note starting at the same tick as an earlier
/// one of the same key replaces it, so ons and offs strictly alternate per key — and sorts
/// automation points by tick (stable, so equal ticks keep their input order: a jump). Each
/// track also gets a note index sorted by start tick (with a prefix maximum of the end ticks)
/// that the player uses to chase notes spanning a position ([`SongTrack::chase`]); the chase
/// flag is part of the track's content hash. Lanes without points are dropped. Device and param
/// ids are not checked here: the engine ignores unknown targets (notes to them are counted as
/// dropped).
pub fn compile_song(input: &SongInput) -> Result<SongSnapshot, SongError> {
    let tempo = compile_tempo_map(&input.tempo)?;
    let meter = compile_meter(&input.meter).map_err(SongError::BadMeterMap)?;
    let mut tracks = Vec::with_capacity(input.tracks.len());
    let mut lanes = Vec::new();
    for track in &input.tracks {
        tracks.push(compile_track(track)?);
        let mut params = std::collections::BTreeSet::new();
        for lane in &track.automation {
            if !params.insert(lane.param) {
                return Err(SongError::BadEvent(format!(
                    "duplicate automation lane for {:?} {:?}",
                    track.device, lane.param
                )));
            }
            if let Some(p) = lane
                .points
                .iter()
                .find(|p| p.tick.0 < 0 || !p.value.is_finite())
            {
                return Err(SongError::BadEvent(format!("automation point {p:?}")));
            }
            if lane.points.is_empty() {
                continue;
            }
            let mut points = lane.points.clone();
            points.sort_by_key(|p| p.tick); // stable
            lanes.push(CompiledLane {
                device: track.device,
                param: lane.param,
                points,
            });
        }
    }
    let audio = crate::audio::compile_audio(&input.audio, &input.assets)?;
    let mut by_key: Vec<_> = tracks
        .iter()
        .enumerate()
        .map(|(k, t)| (t.device, t.socket, k))
        .collect();
    by_key.sort_unstable();
    if let Some(w) = by_key
        .windows(2)
        .find(|w| (w[0].0, w[0].1) == (w[1].0, w[1].1))
    {
        return Err(SongError::DuplicateTrack(w[0].0, w[0].1));
    }
    Ok(SongSnapshot {
        tempo,
        meter,
        audio_rt: (0..audio.len())
            .map(|_| crate::audio::AudioRt::new())
            .collect(),
        audio,
        cursors: vec![TrackCursor::default(); tracks.len()],
        tracks,
        by_key,
        lanes,
    })
}

/// Validates the tempo map and compiles its segment table (`atomik_core::compile_tempo`).
fn compile_tempo_map(map: &[SongTempo]) -> Result<Vec<TempoSegment>, SongError> {
    let points: Vec<TempoPoint> = map
        .iter()
        .map(|t| TempoPoint {
            tick: t.tick.0,
            bpm: t.tempo.bpm(),
            ramp: t.ramp,
        })
        .collect();
    compile_tempo(&points).map_err(|_| SongError::BadTempoMap)
}

/// Validates and flattens one track's notes (see [`compile_song`]).
fn compile_track(track: &SongTrack) -> Result<CompiledTrack, SongError> {
    for n in &track.notes {
        let valid = n.tick.0 >= 0
            && n.length > 0
            && n.key <= 127
            && n.velocity > 0.0
            && n.velocity <= 1.0
            && n.tick.0.checked_add(n.length).is_some();
        if !valid {
            return Err(SongError::BadEvent(format!("note {n:?}")));
        }
    }
    let mut sorted: Vec<&SongNote> = track.notes.iter().collect();
    sorted.sort_by_key(|n| (n.tick, n.key)); // stable: equal (tick, key) keep input order
    // (start, end, key, velocity), with per-key overlaps resolved.
    let mut kept: Vec<(i64, i64, u8, f32)> = Vec::with_capacity(sorted.len());
    let mut last_of_key = [usize::MAX; 128];
    for n in sorted {
        let (start, end, key) = (n.tick.0, n.tick.0 + n.length, n.key);
        let k = usize::from(key);
        if let Some(prev) = kept.get_mut(last_of_key[k])
            && prev.1 > start
        {
            if prev.0 == start {
                *prev = (start, end, key, n.velocity);
                continue;
            }
            prev.1 = start;
        }
        last_of_key[k] = kept.len();
        kept.push((start, end, key, n.velocity));
    }
    let mut ctl: Vec<CtlLane> = Vec::with_capacity(track.controllers.len());
    for c in &track.controllers {
        validate(c).map_err(SongError::BadEvent)?;
        if ctl.iter().any(|l| l.kind == c.kind) {
            return Err(SongError::BadEvent(format!(
                "duplicate controller lane {:?}",
                c.kind
            )));
        }
        ctl.push(expand(c));
    }
    ctl.sort_by_key(|l| l.kind);
    let mut events = Vec::with_capacity(kept.len() * 2);
    for &(start, end, key, velocity) in &kept {
        events.push(SeqEvent {
            tick: start,
            kind: NoteKind::On { key, velocity },
        });
        events.push(SeqEvent {
            tick: end,
            kind: NoteKind::Off { key },
        });
    }
    for lane in &ctl {
        for (&tick, &value) in lane.ticks.iter().zip(&lane.values) {
            events.push(SeqEvent {
                tick,
                kind: lane.kind.note(value),
            });
        }
    }
    // Stable: controllers of one tick keep their lane order, after the offs and before the ons
    // (a bend or a pedal change takes effect for a note that starts with it).
    events.sort_by_key(|e| match e.kind {
        NoteKind::Off { key } => (e.tick, 0, key),
        NoteKind::On { key, .. } => (e.tick, 2, key),
        NoteKind::AllOff => (e.tick, 3, 0),
        // PitchBend, Cc, ChannelPressure and PolyPressure; per-note expressions never come
        // from the song.
        _ => (e.tick, 1, 0),
    });
    let mut hash = mix(
        mix(0x6a09_e667_f3bc_c908, u64::from(track.device.0)),
        u64::from(track.notes_socket.0),
    );
    hash = mix(hash, u64::from(track.chase));
    for e in &events {
        hash = mix(mix(hash, e.tick as u64), encode(e.kind));
    }
    // `kept` is already sorted by (start, key).
    let notes: Vec<ChaseNote> = kept
        .iter()
        .map(|&(start, end, key, velocity)| ChaseNote {
            start,
            end,
            key,
            velocity,
        })
        .collect();
    let mut latest = i64::MIN;
    let max_end = notes
        .iter()
        .map(|n| {
            latest = latest.max(n.end);
            latest
        })
        .collect();
    Ok(CompiledTrack {
        device: track.device,
        socket: track.notes_socket,
        events,
        ctl,
        notes,
        max_end,
        chase: track.chase,
        hash,
    })
}

/// One splitmix64 step folding `value` into `hash` (content hashing; deterministic).
fn mix(hash: u64, value: u64) -> u64 {
    let mut z = (hash ^ value).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn encode(kind: NoteKind) -> u64 {
    match kind {
        NoteKind::On { key, velocity } => {
            1 << 40 | u64::from(key) << 32 | u64::from(velocity.to_bits())
        }
        NoteKind::Off { key } => 2 << 40 | u64::from(key) << 32,
        NoteKind::AllOff => 3 << 40,
        NoteKind::PitchBend { value } => 4 << 40 | u64::from(value.to_bits()),
        NoteKind::Cc { controller, value } => {
            5 << 40 | u64::from(controller) << 32 | u64::from(value.to_bits())
        }
        NoteKind::ChannelPressure { value } => 6 << 40 | u64::from(value.to_bits()),
        NoteKind::PolyPressure { key, value } => {
            7 << 40 | u64::from(key) << 32 | u64::from(value.to_bits())
        }
        NoteKind::NoteExpr { key, kind, value } => {
            8 << 40 | (kind as u64) << 36 | u64::from(key) << 32 | u64::from(value.to_bits())
        }
    }
}

/// Replays the player's loop wrapping: the song frame and loop count `d` frames after (or
/// before, `d < 0`) a batch start at song frame `start` with `loops` wraps so far. `lp` is the
/// active loop `(start frame, end frame)`, `end > start`. See [`SongSnapshot::position_at`].
pub(crate) fn unroll(start: u64, loops: u64, d: i128, lp: Option<(u64, u64)>) -> (u64, u64) {
    let clamp = |x: i128| x.clamp(0, i128::from(u64::MAX)) as u64;
    let Some((ls, le)) = lp else {
        return (clamp(i128::from(start) + d), loops);
    };
    let (ls, le) = (i128::from(ls), i128::from(le));
    let len = le - ls;
    let (mut s, mut lc) = (i128::from(start), i128::from(loops));
    if s > le {
        return (clamp(s + d), loops);
    }
    if s == le {
        // The player wraps at offset 0 of this batch.
        s = ls;
        lc += 1;
    }
    if s < ls {
        let t = s + d;
        if d < 0 || t < le {
            return (clamp(t), clamp(lc));
        }
        let rel = t - le;
        return (clamp(ls + rel % len), clamp(lc + 1 + rel / len));
    }
    let rel = s - ls + d;
    if rel >= 0 {
        return (clamp(ls + rel % len), clamp(lc + rel / len));
    }
    let laps = (-rel + len - 1) / len;
    if laps <= lc {
        (clamp(ls + rel + laps * len), clamp(lc - laps))
    } else {
        (clamp(ls + rel + lc * len), 0)
    }
}

impl SongSnapshot {
    /// Converts a song tick to a song-relative frame position using the tempo map.
    ///
    /// Exact to f64 rounding over any song length: each tempo segment's start time is
    /// accumulated once from integer tick spans, and a position is its segment start plus one
    /// division. Ticks before 0 extend the first tempo backwards.
    pub fn tick_to_frame(&self, tick: Ticks, sample_rate: f64) -> f64 {
        self.map().frame(tick.0 as f64, sample_rate)
    }

    /// Converts a song-relative frame position to a tick using the tempo map. Rounds to the
    /// nearest tick, so it inverts [`SongSnapshot::tick_to_frame`] exactly.
    pub fn frame_to_tick(&self, frame: f64, sample_rate: f64) -> Ticks {
        Ticks(self.frame_to_tick_exact(frame, sample_rate).round() as i64)
    }

    /// Fractional tick at a song frame (the exact inverse of [`SongSnapshot::tick_to_frame`]).
    pub fn frame_to_tick_exact(&self, frame: f64, sample_rate: f64) -> f64 {
        self.map().tick(frame, sample_rate)
    }

    /// The integer song frame at which the player emits events at `tick`:
    /// `tick_to_frame(tick).round()` (ties up); negative ticks give 0.
    pub fn song_frame(&self, tick: Ticks, sample_rate: f64) -> u64 {
        self.map().song_frame(tick.0, sample_rate)
    }

    /// The time signature `(numerator, denominator)` in effect at `tick`.
    pub fn meter_at(&self, tick: Ticks) -> (u8, u8) {
        self.meter_map().meter_at(tick.0)
    }

    /// The tempo at `tick` (inside a ramp, the instantaneous tempo).
    pub fn tempo_at(&self, tick: Ticks) -> Tempo {
        let bpm = self.map().bpm(tick.0 as f64);
        Tempo::new(bpm).unwrap_or_else(|_| unreachable!("segments come from valid tempos"))
    }

    pub(crate) fn map(&self) -> TempoMap<'_> {
        TempoMap(&self.tempo)
    }

    pub(crate) fn meter_map(&self) -> MeterMap<'_> {
        MeterMap(&self.meter)
    }

    /// Converts a live event's engine frame (the `ScheduledNote::frame` timeline) into a song
    /// position, given a [`TransportState`] published by the engine playing this snapshot
    /// (recording). Exact in song frames: it replays the player's loop wrapping from
    /// the state's batch start. `engine_frame` may be before or after `state.engine_frame`; a
    /// frame before it inside the loop is unrolled backwards through at most `loop_count`
    /// earlier laps (then the first lap continues linearly). Assumes the transport kept
    /// playing between the state and the event (no locate or stop in between).
    pub fn position_at(
        &self,
        state: &TransportState,
        engine_frame: u64,
        sample_rate: f64,
    ) -> SongPosition {
        let map = self.map();
        let (ls, le) = (
            map.song_frame(state.loop_start, sample_rate),
            map.song_frame(state.loop_end, sample_rate),
        );
        let lp = (state.loop_enabled && le > ls).then_some((ls, le));
        let d = i128::from(engine_frame) - i128::from(state.engine_frame);
        let (song_frame, loop_count) = unroll(state.song_frame, state.loop_count, d, lp);
        SongPosition {
            song_frame,
            tick: self.frame_to_tick(song_frame as f64, sample_rate),
            loop_count,
        }
    }
}

/// Transport commands (queued in order with other control messages).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransportCommand {
    /// Start playback from the current position.
    Play,
    /// Stop playback (sends AllOff to every track device).
    Stop,
    /// Move the play position (sends AllOff when playing).
    Locate(Ticks),
    /// Set the loop range `[start, end)` and enable flag. A range whose song frames are
    /// empty or reversed never wraps; a position past the end plays on.
    SetLoop {
        /// Loop start.
        start: Ticks,
        /// Loop end (exclusive), > start.
        end: Ticks,
        /// Enabled.
        enabled: bool,
    },
    /// Metronome on/off and level (linear gain, 0..=1), mixed into interface outputs 1/2.
    Metronome {
        /// On.
        enabled: bool,
        /// Gain.
        gain: f32,
    },
}

/// Transport state published by the engine each batch (lock-free; read by UI and recording).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TransportState {
    /// Playing.
    pub playing: bool,
    /// Song tick at the start of the last processed batch.
    pub tick: i64,
    /// Engine host frame at the start of that batch (same timeline as `Engine::host_frames`).
    pub engine_frame: u64,
    /// Number of loop wraps so far (lets recorders detect wraps).
    pub loop_count: u64,
    /// Integer song frame at the start of that batch (`tick` is rounded, this
    /// is exact; see [`SongSnapshot::position_at`]).
    pub song_frame: u64,
    /// Loop start tick in effect.
    pub loop_start: i64,
    /// Loop end tick in effect.
    pub loop_end: i64,
    /// Loop enabled.
    pub loop_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::PPQ;

    const SR: f64 = 48_000.0;

    fn bpm(b: f64) -> Tempo {
        Tempo::new(b).unwrap()
    }

    fn tempo_map(map: &[(i64, f64)]) -> Vec<SongTempo> {
        map.iter()
            .map(|&(tick, b)| SongTempo {
                tick: Ticks(tick),
                tempo: bpm(b),
                ramp: false,
            })
            .collect()
    }

    fn note(tick: i64, length: i64, key: u8) -> SongNote {
        SongNote {
            tick: Ticks(tick),
            length,
            key,
            velocity: 0.5,
        }
    }

    fn track(device: u32, notes: Vec<SongNote>) -> SongTrack {
        SongTrack {
            device: DeviceInstanceId(device),
            notes_socket: SocketId(0),
            chase: true,
            notes,
            automation: Vec::new(),
            controllers: Vec::new(),
        }
    }

    fn point(tick: i64, value: f32) -> SongPoint {
        SongPoint {
            tick: Ticks(tick),
            value,
            curve: SongCurve::Linear,
        }
    }

    fn input(tracks: Vec<SongTrack>, map: &[(i64, f64)]) -> SongInput {
        SongInput {
            tracks,
            tempo: tempo_map(map),
            meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
            audio: Vec::new(),
            assets: Vec::new(),
        }
    }

    fn song(map: &[(i64, f64)]) -> SongSnapshot {
        compile_song(&input(Vec::new(), map)).unwrap()
    }

    fn on(tick: i64, key: u8) -> SeqEvent {
        SeqEvent {
            tick,
            kind: NoteKind::On { key, velocity: 0.5 },
        }
    }

    fn off(tick: i64, key: u8) -> SeqEvent {
        SeqEvent {
            tick,
            kind: NoteKind::Off { key },
        }
    }

    #[test]
    fn compile_rejects_bad_tempo_maps() {
        for map in [
            &[][..],
            &[(10, 120.0)][..],
            &[(0, 120.0), (100, 90.0), (100, 80.0)][..],
            &[(0, 120.0), (100, 90.0), (50, 80.0)][..],
        ] {
            assert_eq!(
                compile_song(&input(Vec::new(), map)).err(),
                Some(SongError::BadTempoMap),
                "{map:?}"
            );
        }
        assert!(compile_song(&input(Vec::new(), &[(0, 120.0), (1, 90.0)])).is_ok());
    }

    #[test]
    fn compile_rejects_bad_meter_maps_and_ramps_to_nowhere() {
        let with_meter = |meter: Vec<MeterEvent>| SongInput {
            meter,
            ..input(Vec::new(), &[(0, 120.0)])
        };
        for meter in [
            vec![],
            vec![MeterEvent::new(2, 4, 4)],
            vec![MeterEvent::new(1, 4, 4), MeterEvent::new(1, 3, 4)],
            vec![MeterEvent::new(1, 0, 4)],
            vec![MeterEvent::new(1, 4, 5)],
        ] {
            assert!(
                matches!(
                    compile_song(&with_meter(meter.clone())),
                    Err(SongError::BadMeterMap(_))
                ),
                "{meter:?}"
            );
        }
        let map = [MeterEvent::new(1, 3, 4), MeterEvent::new(3, 6, 8)];
        let snap = compile_song(&with_meter(map.to_vec())).unwrap();
        assert_eq!(snap.meter_at(Ticks(0)), (3, 4));
        assert_eq!(snap.meter_at(Ticks(6 * PPQ - 1)), (3, 4));
        assert_eq!(snap.meter_at(Ticks(6 * PPQ)), (6, 8));
        assert_eq!(snap.meter_at(Ticks(-5)), (3, 4));
    }

    #[test]
    fn ramp_segments_run_to_the_next_events_tempo_and_convert_both_ways() {
        let bar = 4 * PPQ;
        let ramp = SongInput {
            tempo: vec![
                SongTempo {
                    tick: Ticks(0),
                    tempo: bpm(60.0),
                    ramp: true,
                },
                SongTempo {
                    tick: Ticks(2 * bar),
                    tempo: bpm(180.0),
                    ramp: false,
                },
            ],
            ..input(Vec::new(), &[(0, 120.0)])
        };
        let snap = compile_song(&ramp).unwrap();
        assert_eq!(snap.tempo[0].end_tick, 2 * bar);
        assert_eq!(snap.tempo[0].end_bpm, 180.0);
        assert_eq!(snap.tempo[1].end_tick, i64::MAX);
        assert_eq!(snap.tempo_at(Ticks(0)), bpm(60.0));
        assert_eq!(snap.tempo_at(Ticks(bar)), bpm(120.0));
        assert_eq!(snap.tempo_at(Ticks(2 * bar)), bpm(180.0));
        for tick in [0, 1, PPQ, bar, 2 * bar - 1, 2 * bar, 3 * bar + 7] {
            let f = snap.tick_to_frame(Ticks(tick), SR);
            assert_eq!(snap.frame_to_tick(f, SR), Ticks(tick), "tick {tick}");
        }
    }

    #[test]
    fn compile_rejects_invalid_events_and_tracks() {
        let bad_note = |n: SongNote| input(vec![track(1, vec![n])], &[(0, 120.0)]);
        let lane = |points: Vec<SongPoint>| {
            let mut t = track(1, Vec::new());
            t.automation.push(SongAutomation {
                param: ParamId(0),
                points,
            });
            input(vec![t], &[(0, 120.0)])
        };
        let mut cases = vec![
            bad_note(note(-1, 10, 60)),
            bad_note(note(0, 0, 60)),
            bad_note(note(0, -5, 60)),
            bad_note(note(0, 10, 128)),
            bad_note(note(i64::MAX - 5, 10, 60)),
            lane(vec![point(-1, 0.5)]),
            lane(vec![point(0, f32::NAN)]),
            lane(vec![point(0, f32::INFINITY)]),
        ];
        for velocity in [0.0, -0.5, 1.5, f32::NAN] {
            cases.push(bad_note(SongNote {
                velocity,
                ..note(0, 10, 60)
            }));
        }
        let mut two_lanes = track(1, Vec::new());
        for _ in 0..2 {
            two_lanes.automation.push(SongAutomation {
                param: ParamId(3),
                points: vec![point(0, 0.5)],
            });
        }
        cases.push(input(vec![two_lanes], &[(0, 120.0)]));
        for case in cases {
            assert!(
                matches!(compile_song(&case), Err(SongError::BadEvent(_))),
                "{case:?}"
            );
        }
        let dup = input(
            vec![
                track(4, Vec::new()),
                track(2, Vec::new()),
                track(4, Vec::new()),
            ],
            &[(0, 120.0)],
        );
        assert_eq!(
            compile_song(&dup).err(),
            Some(SongError::DuplicateTrack(DeviceInstanceId(4), SocketId(0)))
        );
        // Same device, another Notes input: fine.
        let mut other = track(4, Vec::new());
        other.notes_socket = SocketId(1);
        assert!(compile_song(&input(vec![track(4, Vec::new()), other], &[(0, 120.0)])).is_ok());
        // Valid edge values.
        let ok = SongNote {
            velocity: 1.0,
            ..note(0, 1, 127)
        };
        assert!(compile_song(&bad_note(ok)).is_ok());
    }

    #[test]
    fn compile_flattens_notes_into_sorted_on_off_events() {
        let notes = vec![
            note(960, 100, 64),
            note(0, 960, 60),
            note(0, 480, 62),
            note(960, 960, 60),
        ];
        let snap = compile_song(&input(vec![track(1, notes)], &[(0, 120.0)])).unwrap();
        assert_eq!(snap.tracks.len(), 1);
        assert_eq!(
            snap.tracks[0].events,
            vec![
                on(0, 60),
                on(0, 62),
                off(480, 62),
                off(960, 60),
                on(960, 60),
                on(960, 64),
                off(1060, 64),
                off(1920, 60),
            ],
            "sorted by (tick, offs before ons, key)"
        );
        assert_eq!(snap.cursors, vec![TrackCursor::default()]);
        assert_eq!(snap.by_key, vec![(DeviceInstanceId(1), SocketId(0), 0)]);
    }

    #[test]
    fn compile_truncates_overlapping_notes_of_one_key() {
        let vel = |n: SongNote, v: f32| SongNote { velocity: v, ..n };
        let notes = vec![
            vel(note(0, 1000, 60), 0.1),
            vel(note(500, 1000, 60), 0.2),
            vel(note(500, 10, 60), 0.3), // same tick and key: replaces the previous one
            vel(note(2000, 1000, 61), 0.4),
            vel(note(2200, 100, 61), 0.5), // contained: cuts the long note
        ];
        let snap = compile_song(&input(vec![track(1, notes)], &[(0, 120.0)])).unwrap();
        let ev = |tick, key, velocity: Option<f32>| SeqEvent {
            tick,
            kind: match velocity {
                Some(velocity) => NoteKind::On { key, velocity },
                None => NoteKind::Off { key },
            },
        };
        assert_eq!(
            snap.tracks[0].events,
            vec![
                ev(0, 60, Some(0.1)),
                ev(500, 60, None),
                ev(500, 60, Some(0.3)),
                ev(510, 60, None),
                ev(2000, 61, Some(0.4)),
                ev(2200, 61, None),
                ev(2200, 61, Some(0.5)),
                ev(2300, 61, None),
            ]
        );
    }

    #[test]
    fn compile_sorts_automation_points_stably_and_hashes_tracks() {
        let mut t = track(2, vec![note(0, 10, 60)]);
        t.automation.push(SongAutomation {
            param: ParamId(1),
            points: vec![point(100, 0.5), point(0, 0.1), point(100, 0.7)],
        });
        let snap = compile_song(&input(vec![t.clone()], &[(0, 120.0)])).unwrap();
        assert_eq!(snap.lanes.len(), 1);
        assert_eq!(snap.lanes[0].device, DeviceInstanceId(2));
        assert_eq!(snap.lanes[0].param, ParamId(1));
        assert_eq!(
            snap.lanes[0].points,
            vec![point(0, 0.1), point(100, 0.5), point(100, 0.7)]
        );
        // Hash: equal content, equal hash (the tempo map is not part of it); a note change
        // gives another hash.
        let again = compile_song(&input(vec![t.clone()], &[(0, 90.0)])).unwrap();
        assert_eq!(snap.tracks[0].hash, again.tracks[0].hash);
        t.notes[0].key = 61;
        let changed = compile_song(&input(vec![t], &[(0, 120.0)])).unwrap();
        assert_ne!(snap.tracks[0].hash, changed.tracks[0].hash);
    }

    #[test]
    fn compile_hash_includes_the_chase_flag() {
        let on = track(2, vec![note(0, 10, 60)]);
        let mut off = on.clone();
        off.chase = false;
        let a = compile_song(&input(vec![on], &[(0, 120.0)])).unwrap();
        let b = compile_song(&input(vec![off], &[(0, 120.0)])).unwrap();
        assert_ne!(a.tracks[0].hash, b.tracks[0].hash);
        assert!(a.tracks[0].chase && !b.tracks[0].chase);
    }

    #[test]
    fn compile_builds_the_chase_index_from_the_resolved_notes() {
        // Key 60 overlaps itself (the first is cut at 50); a long note starts before a short one.
        let notes = vec![
            note(50, 100, 60),
            note(0, 100, 60),
            note(10, 500, 62),
            note(20, 30, 64),
        ];
        let snap = compile_song(&input(vec![track(1, notes)], &[(0, 120.0)])).unwrap();
        let t = &snap.tracks[0];
        let got: Vec<_> = t.notes.iter().map(|n| (n.start, n.end, n.key)).collect();
        assert_eq!(
            got,
            vec![(0, 50, 60), (10, 510, 62), (20, 50, 64), (50, 150, 60)]
        );
        assert_eq!(t.max_end, vec![50, 510, 510, 510]);
    }

    #[test]
    fn tempo_segments_convert_ticks_and_frames_piecewise() {
        let bar = 4 * PPQ;
        let snap = song(&[(0, 120.0), (bar, 60.0), (2 * bar, 240.0)]);
        let seg = |tick, seconds, bpm, end_tick, end_bpm| TempoSegment {
            tick,
            seconds,
            bpm,
            end_tick,
            end_bpm,
        };
        assert_eq!(
            snap.tempo,
            vec![
                seg(0, 0.0, 120.0, bar, 120.0),
                seg(bar, 2.0, 60.0, 2 * bar, 60.0),
                seg(2 * bar, 6.0, 240.0, i64::MAX, 240.0)
            ]
        );
        let cases = [
            (0, 0.0),
            (PPQ, 24_000.0),
            (bar, 96_000.0),
            (bar + PPQ, 144_000.0),
            (2 * bar, 288_000.0),
            (2 * bar + PPQ, 300_000.0),
            (-PPQ, -24_000.0), // before tick 0: the first tempo extends backwards
        ];
        for (tick, frame) in cases {
            assert_eq!(snap.tick_to_frame(Ticks(tick), SR), frame, "tick {tick}");
            assert_eq!(snap.frame_to_tick(frame, SR), Ticks(tick), "frame {frame}");
            assert_eq!(snap.frame_to_tick_exact(frame, SR), tick as f64);
        }
        assert_eq!(snap.tempo_at(Ticks(0)), bpm(120.0));
        assert_eq!(snap.tempo_at(Ticks(bar - 1)), bpm(120.0));
        assert_eq!(snap.tempo_at(Ticks(bar)), bpm(60.0));
        assert_eq!(snap.tempo_at(Ticks(10 * bar)), bpm(240.0));
        assert_eq!(snap.tempo_at(Ticks(-1)), bpm(120.0));
        // frame_to_tick rounds to the nearest tick (1 tick = 1.5625 frames at 120 BPM).
        assert_eq!(snap.frame_to_tick(1.0, SR), Ticks(1));
        assert_eq!(snap.frame_to_tick(0.7, SR), Ticks(0));
        assert_eq!(snap.frame_to_tick_exact(1.5625, SR), 1.0);
    }

    #[test]
    fn song_frame_rounds_to_the_nearest_frame() {
        let snap = song(&[(0, 120.0)]); // 1.5625 frames per tick
        assert_eq!(snap.song_frame(Ticks(0), SR), 0);
        assert_eq!(snap.song_frame(Ticks(1), SR), 2); // 1.5625
        assert_eq!(snap.song_frame(Ticks(2), SR), 3); // 3.125
        assert_eq!(snap.song_frame(Ticks(8), SR), 13); // 12.5: ties go up
        assert_eq!(snap.song_frame(Ticks(PPQ), SR), 24_000);
        assert_eq!(snap.song_frame(Ticks(-5), SR), 0);
        let mut last = 0;
        for t in 0..10_000 {
            let f = snap.song_frame(Ticks(t), SR);
            assert!(f >= last);
            assert_eq!(f, snap.tick_to_frame(Ticks(t), SR).round() as u64);
            last = f;
        }
    }

    /// Tick <-> frame stays exact over long songs (64 bars and 3 hours) with several
    /// tempo changes: the frame matches an exact rational computation, and frame_to_tick
    /// inverts tick_to_frame.
    #[test]
    fn tick_frame_conversion_is_drift_free_at_64_bars_and_3_hours() {
        let bar = 4 * PPQ;
        // Integer BPMs so the exact value is a rational: frames = 187.5 * sum(dt_k / bpm_k).
        let changes = [
            (0, 120),
            (3 * bar, 97),
            (17 * bar, 143),
            (40 * bar, 60),
            (63 * bar, 133),
        ];
        let map: Vec<(i64, f64)> = changes.iter().map(|&(t, b)| (t, f64::from(b))).collect();
        let snap = song(&map);
        let lcm = changes.iter().fold(1i128, |acc, &(_, b)| {
            let b = i128::from(b);
            let (mut x, mut y) = (acc, b);
            while y != 0 {
                (x, y) = (y, x % y);
            }
            acc / x * b
        });
        let exact = |tick: i64| -> f64 {
            let mut num: i128 = 0;
            for (k, &(start, b)) in changes.iter().enumerate() {
                let end = changes.get(k + 1).map_or(i64::MAX, |c| c.0);
                if tick > start {
                    let dt = i128::from(tick.min(end) - start);
                    num += dt * (lcm / i128::from(b));
                }
            }
            // 187.5 = 48000 * 60 / 15360.
            (num as f64) * 187.5 / lcm as f64
        };
        let three_hours = {
            // Ticks after the last change (133 BPM) up to 3 h.
            let t_last = exact(63 * bar) / SR;
            63 * bar + ((3.0 * 3600.0 - t_last) * 133.0 / 60.0 * PPQ as f64) as i64
        };
        assert!(snap.tick_to_frame(Ticks(three_hours), SR) > 3.0 * 3600.0 * SR - SR);
        let mut ticks: Vec<i64> = (0..=64 * 16).map(|k| k * PPQ / 4 + k % 7).collect();
        ticks.extend((0..2000).map(|k| three_hours / 2000 * k + (k * 7919) % PPQ));
        ticks.push(three_hours);
        for t in ticks {
            let f = snap.tick_to_frame(Ticks(t), SR);
            assert!((f - exact(t)).abs() < 1e-5, "tick {t}: {f} vs {}", exact(t));
            assert_eq!(
                snap.frame_to_tick(f, SR),
                Ticks(t),
                "round trip at tick {t}"
            );
            let back = snap.frame_to_tick_exact(f, SR);
            assert!((back - t as f64).abs() < 1e-4, "tick {t}: {back}");
        }
    }

    fn state(song_frame: u64, engine_frame: u64, loop_count: u64) -> TransportState {
        TransportState {
            playing: true,
            tick: 0,
            engine_frame,
            loop_count,
            song_frame,
            loop_start: 0,
            loop_end: 0,
            loop_enabled: false,
        }
    }

    fn pos(song_frame: u64, tick: i64, loop_count: u64) -> SongPosition {
        SongPosition {
            song_frame,
            tick: Ticks(tick),
            loop_count,
        }
    }

    #[test]
    fn position_at_without_loop_is_linear() {
        let snap = song(&[(0, 120.0)]);
        let s = state(24_000, 6_400, 3);
        assert_eq!(snap.position_at(&s, 6_400, SR), pos(24_000, PPQ, 3));
        assert_eq!(snap.position_at(&s, 30_400, SR), pos(48_000, 2 * PPQ, 3));
        assert_eq!(snap.position_at(&s, 400, SR), pos(18_000, 11_520, 3));
        assert_eq!(snap.position_at(&s, 0, SR), pos(17_600, 11_264, 3));
        // Before the song start: clamped at frame 0.
        let early = state(100, 6_400, 0);
        assert_eq!(snap.position_at(&early, 0, SR), pos(0, 0, 0));
        // A disabled loop does not wrap.
        let off = TransportState {
            loop_start: 0,
            loop_end: PPQ,
            ..s
        };
        assert_eq!(snap.position_at(&off, 30_400, SR), pos(48_000, 2 * PPQ, 3));
    }

    #[test]
    fn position_at_unrolls_loop_wraps_both_ways() {
        let snap = song(&[(0, 120.0)]);
        // Loop [1 beat, 2 beats) = song frames [24000, 48000).
        let looped = |song_frame, engine_frame, loop_count| TransportState {
            loop_start: PPQ,
            loop_end: 2 * PPQ,
            loop_enabled: true,
            ..state(song_frame, engine_frame, loop_count)
        };
        let s = looped(47_990, 1_000, 2);
        assert_eq!(snap.position_at(&s, 1_009, SR), pos(47_999, 30_719, 2));
        assert_eq!(snap.position_at(&s, 1_010, SR), pos(24_000, PPQ, 3));
        assert_eq!(snap.position_at(&s, 1_015, SR), pos(24_005, PPQ + 3, 3));
        assert_eq!(
            snap.position_at(&s, 1_010 + 2 * 24_000 + 1, SR),
            pos(24_001, PPQ + 1, 5)
        );
        // Backwards across a wrap, limited to loop_count laps.
        let s = looped(24_005, 100_015, 3);
        assert_eq!(snap.position_at(&s, 100_010, SR), pos(24_000, PPQ, 3));
        assert_eq!(snap.position_at(&s, 100_009, SR), pos(47_999, 30_719, 2));
        assert_eq!(
            snap.position_at(&s, 100_010 - 3 * 24_000, SR),
            pos(24_000, PPQ, 0)
        );
        assert_eq!(
            snap.position_at(&s, 100_010 - 3 * 24_000 - 1, SR),
            pos(23_999, 15_359, 0),
            "beyond loop_count laps the first lap continues linearly"
        );
        // A state exactly at the loop end has not wrapped yet: its first frame is the start.
        let s = looped(48_000, 2_000, 1);
        assert_eq!(snap.position_at(&s, 2_000, SR), pos(24_000, PPQ, 2));
        assert_eq!(snap.position_at(&s, 1_999, SR), pos(47_999, 30_719, 1));
        // From before the loop: linear until the loop end, then wraps.
        let s = looped(0, 0, 0);
        assert_eq!(snap.position_at(&s, 47_999, SR), pos(47_999, 30_719, 0));
        assert_eq!(snap.position_at(&s, 48_000, SR), pos(24_000, PPQ, 1));
        // From after the loop end: never wraps.
        let s = looped(60_000, 0, 4);
        assert_eq!(snap.position_at(&s, 100, SR), pos(60_100, 38_464, 4));
        // An empty or reversed loop range never wraps.
        let s = TransportState {
            loop_end: PPQ,
            ..looped(0, 0, 0)
        };
        assert_eq!(snap.position_at(&s, 30_000, SR), pos(30_000, 19_200, 0));
    }
}
