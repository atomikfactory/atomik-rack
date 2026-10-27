//! Song data and song commands.
//!
//! All times are in ticks ([`atomik_core::PPQ`] per quarter note). The song references rack
//! devices by [`DeviceKey`]; a track belongs to exactly one device with a Notes input.
//!
//! Public types are a stable contract: changes need review.

use crate::DeviceKey;
use crate::automation_rec::WriteMode;
use crate::controller::ControllerLane;
use crate::freeze::Frozen;
use crate::groove::{Groove, QuantizeParams, builtin_groove};
use crate::organise::Organisation;
use crate::takes::TakeGroup;
use crate::theory::Scale;
use crate::warp::WarpSettings;
pub use atomik_core::{BEAT_UNITS, MAX_BEATS_PER_BAR, MeterEvent};
use atomik_core::{MeterTable, Tempo, TempoPoint, TempoTable};
use std::collections::BTreeMap;

/// Stable, unique id of a clip within a song (never reused while the song lives).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClipId(pub u64);

/// One note in a note clip, relative to the clip's content start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    /// Start tick relative to the clip content (0 = clip content start).
    pub tick: i64,
    /// Length in ticks (> 0).
    pub length: i64,
    /// MIDI key 0..=127.
    pub key: u8,
    /// Velocity in (0, 1].
    pub velocity: f32,
    /// A muted note stays in the clip (and the editors) but is never played.
    pub muted: bool,
}

impl Note {
    /// An unmuted note.
    pub const fn new(tick: i64, length: i64, key: u8, velocity: f32) -> Self {
        Self {
            tick,
            length,
            key,
            velocity,
            muted: false,
        }
    }
}

/// Interpolation from one automation point to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    /// Linear ramp to the next point.
    Linear,
    /// Hold this value until the next point.
    Step,
}

/// One automation point, relative to the clip content start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutomationPoint {
    /// Tick relative to the clip content.
    pub tick: i64,
    /// Plain parameter value (clamped by the descriptor).
    pub value: f32,
    /// Segment shape towards the next point.
    pub curve: Curve,
}

/// The shape of an audio fade (and of a crossfade).
///
/// `eval(t)` is the gain of a fade-in at position `t` in `[0, 1]` (0 at its start, 1 at its
/// end); a fade-out mirrors it (`eval(1 - t)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeCurve {
    /// Gain ramps linearly with position.
    Linear,
    /// Gain follows a quarter sine (`sin(t * pi / 2)` in, mirrored out), so two crossfading
    /// clips keep constant power.
    EqualPower,
    /// Gain follows `t * t`: a slow start and a fast finish in, the natural decay out.
    Exponential,
    /// Gain follows `t * t * (3 - 2 * t)`: slow at both ends, fastest in the middle.
    SCurve,
}

impl FadeCurve {
    /// Every curve, in menu order.
    pub const ALL: [FadeCurve; 4] = [
        FadeCurve::Linear,
        FadeCurve::EqualPower,
        FadeCurve::Exponential,
        FadeCurve::SCurve,
    ];

    /// The reference gain at position `t` (clamped to `[0, 1]`). The engine evaluates the same
    /// curves with platform-independent arithmetic (`sin_quarter` for the quarter sine), so it
    /// differs from this only in the last bit of an equal-power fade.
    pub fn eval(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            FadeCurve::Linear => t,
            FadeCurve::EqualPower => (t * std::f64::consts::FRAC_PI_2).sin(),
            FadeCurve::Exponential => t * t,
            FadeCurve::SCurve => t * t * (3.0 - 2.0 * t),
        }
    }

    /// The name used in the project file and by the commands.
    pub fn id(self) -> &'static str {
        match self {
            FadeCurve::Linear => "linear",
            FadeCurve::EqualPower => "equal_power",
            FadeCurve::Exponential => "exponential",
            FadeCurve::SCurve => "s_curve",
        }
    }

    /// The curve called `id` (see [`FadeCurve::id`]).
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    /// A short label for menus and the inspector.
    pub fn label(self) -> &'static str {
        match self {
            FadeCurve::Linear => "Linear",
            FadeCurve::EqualPower => "Equal power",
            FadeCurve::Exponential => "Exponential",
            FadeCurve::SCurve => "S-curve",
        }
    }
}

/// One breakpoint of a clip's gain envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainPoint {
    /// Frames from the clip start (audio does not stretch, so it is a time, like a fade).
    pub frame: u64,
    /// Gain in dB at that frame ([`AUDIO_GAIN_DB_MIN`]..=[`AUDIO_GAIN_DB_MAX`]); it multiplies
    /// the clip gain and the fades.
    pub gain_db: f32,
}

/// Most breakpoints of one gain envelope.
pub const MAX_GAIN_POINTS: usize = 512;

/// What an audio clip plays.
///
/// The clip's `start` and `length` stay in ticks; playback runs from `source_offset` for the
/// clip's duration in real time (no stretch unless [`AudioClipContent::warp`] says so), cut at
/// the clip end or the asset end. The
/// clip's own `offset` is always 0 and its `loop_length` always `None`.
///
/// The fades, the gain envelope and the reverse flag all work in *clip time* (frames from the
/// clip start), so reversing a clip leaves its fades and envelope where they are.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioClipContent {
    /// Asset id in [`crate::RackDoc::assets`] (64 lowercase hex digits).
    pub asset: String,
    /// First asset frame played (`<=` the asset's frame count); with `reverse` the lowest
    /// frame played (the clip plays the frames from here up to `source_offset + length`
    /// backwards).
    pub source_offset: u64,
    /// Clip gain in dB (finite, [`AUDIO_GAIN_DB_MIN`]..=[`AUDIO_GAIN_DB_MAX`]).
    pub gain_db: f32,
    /// Fade-in length in frames, from the clip start.
    pub fade_in: u64,
    /// Fade-out length in frames, ending at the clip end.
    pub fade_out: u64,
    /// Shape of both fades.
    pub fade_curve: FadeCurve,
    /// Plays the clip's span of the asset backwards.
    pub reverse: bool,
    /// Gain breakpoints in clip time: strictly increasing frames, at most
    /// [`MAX_GAIN_POINTS`]; empty means no envelope. Linear between points, held before the
    /// first and after the last.
    pub envelope: Vec<GainPoint>,
    /// The crossfade this clip makes with the clip it overlaps at its start (see
    /// [`flatten_audio`]): `Some(curve)` crossfades, `None` is a hard cut.
    pub xfade: Option<FadeCurve>,
    /// Warp: follow the song tempo and transpose without changing length (see
    /// [`crate::warp`]). The default plays the asset in real time. A reversed clip cannot be
    /// warped.
    pub warp: WarpSettings,
}

impl AudioClipContent {
    /// A plain clip of `asset` from `source_offset`: no gain, fades, envelope or reverse, and
    /// an equal-power crossfade where it overlaps an earlier clip.
    pub fn new(asset: impl Into<String>, source_offset: u64) -> Self {
        Self {
            asset: asset.into(),
            source_offset,
            gain_db: 0.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: FadeCurve::Linear,
            reverse: false,
            envelope: Vec::new(),
            xfade: Some(FadeCurve::EqualPower),
            warp: WarpSettings::default(),
        }
    }

    /// The envelope's linear gain at clip frame `n`, or `None` without an envelope. This is
    /// the reference the engine follows: the points' dB values become amplitudes and the
    /// gain ramps linearly in amplitude between them (a drawn dB curve bends a little between
    /// two points for that reason), held before the first and after the last.
    pub fn envelope_gain_at(&self, n: u64) -> Option<f32> {
        envelope_gain(&self.envelope_linear(), n)
    }

    /// The envelope as `(frame, linear gain)` pairs.
    pub fn envelope_linear(&self) -> Vec<(u64, f32)> {
        self.envelope
            .iter()
            .map(|p| (p.frame, atomik_core::db_to_gain(p.gain_db)))
            .collect()
    }
}

/// The reference value of a gain envelope given as `(frame, linear gain)` pairs at frame `n`
/// (see [`AudioClipContent::envelope_gain_at`]); evaluated in `f64`, rounded to `f32`.
pub fn envelope_gain(points: &[(u64, f32)], n: u64) -> Option<f32> {
    let first = points.first()?;
    let k = points.partition_point(|q| q.0 <= n);
    if k == 0 {
        return Some(first.1);
    }
    let a = points[k - 1];
    let Some(b) = points.get(k) else {
        return Some(a.1);
    };
    let t = (n - a.0) as f64 / (b.0 - a.0) as f64;
    Some((f64::from(a.1) + (f64::from(b.1) - f64::from(a.1)) * t) as f32)
}

/// What a clip contains.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipContent {
    /// Notes, sorted by (tick, key).
    Notes(Vec<Note>),
    /// Automation points, sorted by tick, for the lane's parameter.
    Automation(Vec<AutomationPoint>),
    /// Audio from the sample pool (audio lanes only).
    Audio(AudioClipContent),
}

/// A clip placed on a lane.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// Stable id.
    pub id: ClipId,
    /// Song tick where the clip starts.
    pub start: i64,
    /// Clip length in ticks (> 0).
    pub length: i64,
    /// Content offset in ticks (content tick shown at the clip start).
    pub offset: i64,
    /// If `Some(n)`, the content loops every `n` ticks inside the clip.
    pub loop_length: Option<i64>,
    /// Muted clips do not play.
    pub muted: bool,
    /// The step grid: `Some` only on a note clip (the model rejects it
    /// on audio and automation clips). It drives swing in [`flatten_notes`] and the step
    /// editor; it never changes the clip's length or notes.
    pub grid: Option<StepGrid>,
    /// The scale the note editors highlight and snap to: `Some` only on a note clip and never
    /// chromatic (no scale is `None`). It never changes the clip's notes or how they play.
    pub scale: Option<Scale>,
    /// Pitch bend, aftertouch and controller lanes, in content ticks: non-empty only on a note
    /// clip (see [`crate::controller`]). Sorted by kind, one lane per kind. They follow the
    /// clip's offset, loop and audible span like its notes do.
    pub controllers: Vec<ControllerLane>,
    /// Content.
    pub content: ClipContent,
}

/// The length of one step of a [`StepGrid`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StepResolution {
    /// 1/8 note: `PPQ / 2` ticks.
    Eighth,
    /// 1/16 note: `PPQ / 4` ticks.
    Sixteenth,
    /// 1/32 note: `PPQ / 8` ticks.
    ThirtySecond,
}

impl StepResolution {
    /// The step length in ticks: 7 680, 3 840 or 1 920 at PPQ 15 360.
    pub fn ticks(self) -> i64 {
        match self {
            StepResolution::Eighth => atomik_core::PPQ / 2,
            StepResolution::Sixteenth => atomik_core::PPQ / 4,
            StepResolution::ThirtySecond => atomik_core::PPQ / 8,
        }
    }
}

/// Fewest steps of a [`StepGrid`].
pub const GRID_STEPS_MIN: u16 = 16;
/// Most steps of a [`StepGrid`].
pub const GRID_STEPS_MAX: u16 = 64;
/// Largest swing of a [`StepGrid`], in percent.
pub const GRID_SWING_MAX: u8 = 75;

/// A step grid on a note clip: the step length, the number of steps the step editor
/// shows and the swing. Swing delays the notes in odd steps; see [`flatten_notes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepGrid {
    /// Step length.
    pub resolution: StepResolution,
    /// Number of steps, [`GRID_STEPS_MIN`]..=[`GRID_STEPS_MAX`].
    pub steps: u16,
    /// Swing in percent of half a step, 0..=[`GRID_SWING_MAX`].
    pub swing: u8,
}

impl StepGrid {
    /// Checks the ranges; `Err` says what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        if !(GRID_STEPS_MIN..=GRID_STEPS_MAX).contains(&self.steps) {
            return Err(format!(
                "grid steps {} is outside {GRID_STEPS_MIN}..={GRID_STEPS_MAX}",
                self.steps
            ));
        }
        if self.swing > GRID_SWING_MAX {
            return Err(format!(
                "grid swing {} is outside 0..={GRID_SWING_MAX}",
                self.swing
            ));
        }
        Ok(())
    }

    /// The swing delay in ticks of a note in an odd step: `(swing * s + 100) / 200` with
    /// `s` the step length (swing percent of half a step, rounded half up).
    pub fn swing_ticks(&self) -> i64 {
        (i64::from(self.swing) * self.resolution.ticks() + 100) / 200
    }

    /// The swung start of a note whose content tick is `t`: `t` plus [`StepGrid::swing_ticks`]
    /// when `t mod 2s` lies in `[s, 2s)`, else `t`.
    pub fn swung(&self, t: i64) -> i64 {
        let s = self.resolution.ticks();
        if t.rem_euclid(2 * s) >= s {
            t + self.swing_ticks()
        } else {
            t
        }
    }
}

/// A lane: the note lane, or an automation lane for one parameter of the track's device.
#[derive(Debug, Clone, PartialEq)]
pub enum LaneKind {
    /// Notes into the device's notes input.
    Notes,
    /// Automation of the parameter with this key.
    Automation(String),
    /// Audio clips (the only lane of a track on an `atomik.audio-track` device).
    Audio,
}

/// The most note lanes a track can have.
pub const MAX_NOTE_LANES: usize = 64;

/// The longest note lane name, in characters.
pub const MAX_LANE_NAME_CHARS: usize = 32;

/// A lane with its clips (sorted by start; clips may overlap: later clips win).
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    /// Lane kind.
    pub kind: LaneKind,
    /// The name of a note lane (`None` shows the default, see [`Track::lane_label`]). Always
    /// `None` on automation and audio lanes.
    pub name: Option<String>,
    /// A muted note lane plays nothing (its clips stay and stay editable). Always `false` on
    /// automation and audio lanes.
    pub muted: bool,
    /// Clips, sorted by (start, id).
    pub clips: Vec<Clip>,
    /// Stacked takes and their comps (notes and audio lanes only; see [`crate::takes`]). Each
    /// group's compiled comp clips are in `clips`; its takes are not.
    pub take_groups: Vec<TakeGroup>,
}

/// A track bound to one rack device.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// The device this track plays.
    pub device: DeviceKey,
    /// Track mute.
    pub muted: bool,
    /// Track solo.
    pub solo: bool,
    /// Armed for recording.
    pub armed: bool,
    /// Notes that span the play position start sounding on play, locate and loop wrap. On by
    /// default; a track that plays only from its note starts turns it off.
    pub chase: bool,
    /// Where the track's notes enter, when not at its own device: the top of a stack of note
    /// players above the instrument (see [`crate::WorkflowCommand::CreatePlayer`]). `None` (the
    /// default, and the only value of a track written before project format 31) is the track's
    /// own device. The track stays the instrument's: its channel, its automation and its freeze.
    /// A device that is not in the document any more counts as `None`
    /// ([`crate::RackDoc::notes_entry`]).
    pub notes_to: Option<DeviceKey>,
    /// The track is frozen: its instrument and the first effects of its channel's insert chain
    /// are rendered to the audio held here, which plays instead of them (see
    /// [`crate::freeze`]). The clips, the instrument and the effects stay as they were.
    pub frozen: Option<Frozen>,
    /// Lanes: the note lanes first (at least one, up to [`MAX_NOTE_LANES`]; lane 0 is "the"
    /// note lane), then the automation lanes; an audio track has its audio lane alone. All the
    /// note lanes play the track's device.
    pub lanes: Vec<Lane>,
}

/// A tempo change at a tick: a step to `bpm`, or with `ramp` a linear ramp (in BPM, over ticks)
/// to the next event's BPM. `ramp` on the last event has no meaning and is rejected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoEvent {
    /// Tick where the tempo takes effect.
    pub tick: i64,
    /// Tempo in BPM (20..=999).
    pub bpm: f64,
    /// The tempo ramps linearly to the next event's BPM instead of holding.
    pub ramp: bool,
}

impl TempoEvent {
    /// A step to `bpm` at `tick`.
    pub const fn step(tick: i64, bpm: f64) -> Self {
        Self {
            tick,
            bpm,
            ramp: false,
        }
    }
}

/// The song: tracks, tempo map, meter, loop.
#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    /// Tracks in display order.
    pub tracks: Vec<Track>,
    /// Tempo changes, sorted by tick; the first is at tick 0 and mirrors `RackDoc::tempo_bpm`.
    pub tempo: Vec<TempoEvent>,
    /// Meter changes, sorted by bar; the first is at bar 1. Changes happen on bar lines.
    pub meter: Vec<MeterEvent>,
    /// Loop range `[start, end)` in ticks.
    pub loop_range: (i64, i64),
    /// Loop enabled.
    pub loop_enabled: bool,
    /// Song End (the length of a render and of the arrangement), in ticks; see
    /// [`Song::end_tick`]. A clip drawn, moved or stretched past it moves it out to the next
    /// bar line, in the same undo step.
    pub end: i64,
    /// Next clip id to allocate.
    pub next_clip_id: u64,
    /// Point markers, sorted by `(tick, id)`. Ticks, not time: they follow the bars when the
    /// tempo changes.
    pub markers: Vec<Marker>,
    /// Named ranges (sections), sorted by `(start, id)`; they may overlap.
    pub ranges: Vec<RangeMarker>,
    /// Height of a track's first lane in the arrangement, in pixels at vertical zoom 1 (view
    /// state saved with the project): `MIN_TRACK_HEIGHT..=MAX_TRACK_HEIGHT`. A track without an
    /// entry has `DEFAULT_TRACK_HEIGHT`. Entries of devices without a track are dropped.
    pub track_heights: BTreeMap<DeviceKey, u16>,
    /// The write mode of an automation lane, by `(device, param)`: how a recording pass writes
    /// into it. A lane without an entry has the default mode, [`WriteMode::Touch`] (an entry
    /// never holds the default). Entries of lanes that do not exist are dropped when saving.
    pub lane_write: BTreeMap<(DeviceKey, String), WriteMode>,
    /// The song's groove templates, shared by every track (see [`crate::groove`]); the built-in
    /// grooves are not stored here. Names are unique and never a built-in name.
    pub grooves: Vec<Groove>,
    /// Folders of tracks, track colours and comments, folded lanes (see [`crate::organise`]).
    pub org: Organisation,
}

/// A named point in the song. The id is only for telling markers apart while editing; files do
/// not store it (a loaded song numbers its markers from 1 in file order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    /// Unique among the markers.
    pub id: u32,
    /// Song tick, `0..=MAX_TICK`.
    pub tick: i64,
    /// Trimmed, non-empty, at most [`MAX_MARKER_NAME`] characters.
    pub name: String,
    /// Optional colour.
    pub color: Option<[u8; 3]>,
}

/// A named section `[start, end)` of the song (Intro, Verse, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeMarker {
    /// Unique among the ranges.
    pub id: u32,
    /// Start tick, `0..end`.
    pub start: i64,
    /// End tick, at most `MAX_TICK`.
    pub end: i64,
    /// Trimmed, non-empty, at most [`MAX_MARKER_NAME`] characters.
    pub name: String,
    /// Optional colour.
    pub color: Option<[u8; 3]>,
}

/// Shortest track height, in pixels.
pub const MIN_TRACK_HEIGHT: u16 = 24;
/// Tallest track height, in pixels.
pub const MAX_TRACK_HEIGHT: u16 = 400;
/// Height of a track without an entry in [`Song::track_heights`].
pub const DEFAULT_TRACK_HEIGHT: u16 = 60;

/// Longest marker or range name, in characters.
pub const MAX_MARKER_NAME: usize = 64;

/// Song edits (wrapped in `Command::Song`). Every command is undoable.
#[derive(Debug, Clone, PartialEq)]
pub enum SongCommand {
    /// Create the track for a device (error if it exists, or if the device has no Notes input
    /// and is not an `atomik.audio-track`). An audio-track device gets an audio lane at index 0
    /// (no cables are made; see [`SongCommand::AddAudioTrack`]).
    AddTrack {
        /// Device.
        device: DeviceKey,
    },
    /// Remove a device's track (its clips go to undo history).
    RemoveTrack {
        /// Device.
        device: DeviceKey,
    },
    /// Set mute/solo/arm.
    SetTrackFlags {
        /// Device.
        device: DeviceKey,
        /// Mute.
        muted: bool,
        /// Solo.
        solo: bool,
        /// Armed.
        armed: bool,
    },
    /// Set whether a track chases notes (see [`Track::chase`]).
    SetTrackChase {
        /// Device.
        device: DeviceKey,
        /// Chase notes that span the play position.
        chase: bool,
    },
    /// Add a note lane after the track's last note lane (before its automation lanes), with a
    /// name (trimmed; blank or `None` leaves it unnamed). One undo step. The track must exist,
    /// not be an audio track and not be frozen, and has at most [`MAX_NOTE_LANES`] note lanes.
    AddNoteLane {
        /// Device (track).
        device: DeviceKey,
        /// The name, at most [`MAX_LANE_NAME_CHARS`] characters without control characters.
        name: Option<String>,
    },
    /// Remove a note lane with its clips and takes (one undo step). The track keeps at least
    /// one note lane.
    RemoveNoteLane {
        /// Device (track).
        device: DeviceKey,
        /// Index of the note lane.
        lane: usize,
    },
    /// Rename a note lane (see [`Lane::name`]); blank or `None` resets it to the default. One
    /// undo step.
    RenameLane {
        /// Device (track).
        device: DeviceKey,
        /// Index of the note lane.
        lane: usize,
        /// The name, trimmed, at most [`MAX_LANE_NAME_CHARS`] characters without control
        /// characters.
        name: Option<String>,
    },
    /// Mute or unmute a note lane (see [`Lane::muted`]); setting the value it has is an error.
    /// One undo step.
    SetLaneMuted {
        /// Device (track).
        device: DeviceKey,
        /// Index of the note lane.
        lane: usize,
        /// The new state.
        muted: bool,
    },
    /// Set the write mode of an automation lane (see [`Song::lane_write`]); the lane must
    /// exist. One undo step. Reports [`crate::SyncOp::ViewChanged`] only: the engine never sees
    /// it.
    SetLaneWriteMode {
        /// Device.
        device: DeviceKey,
        /// Param key of the lane.
        param: String,
        /// The mode.
        mode: WriteMode,
    },
    /// Put tracks in a new folder (see [`crate::organise`]). The tracks must exist, be listed
    /// once each and share their innermost folder (or have none); the new folder goes inside
    /// that one. They are moved next to each other at the place of the first of them (in track
    /// order). The new folder's id is the highest one afterwards ([`Song::org`]). One undo
    /// step. Reports [`crate::SyncOp::SongChanged`] when tracks move, else
    /// [`crate::SyncOp::ViewChanged`].
    GroupTracks {
        /// The tracks.
        devices: Vec<DeviceKey>,
        /// The folder's name: trimmed, non-empty, at most [`crate::organise::MAX_GROUP_NAME`]
        /// characters.
        name: String,
    },
    /// Dissolve a folder: its tracks and sub-folders go into its parent (or the top level),
    /// where they stand. No track is deleted. One undo step.
    Ungroup {
        /// Folder id.
        group: u32,
    },
    /// Rename a folder (the name rules of [`SongCommand::GroupTracks`]).
    RenameGroup {
        /// Folder id.
        group: u32,
        /// New name.
        name: String,
    },
    /// Collapse or expand a folder in the arrangement.
    SetGroupCollapsed {
        /// Folder id.
        group: u32,
        /// Hide the folder's tracks.
        collapsed: bool,
    },
    /// Set or clear a folder's colour.
    SetGroupColor {
        /// Folder id.
        group: u32,
        /// The colour.
        color: Option<[u8; 3]>,
    },
    /// Move a track one place up or down the track list. A track passes its neighbour; at the
    /// edge of a folder it leaves the folder, and above or below a folder it enters it (as the
    /// last or first track), so two keys reach every position. An error at the first and last
    /// place of the top level. One undo step.
    MoveTrack {
        /// Device.
        device: DeviceKey,
        /// Down (later in the list) instead of up.
        down: bool,
    },
    /// Move a folder, with everything in it, one place up or down among its neighbours (a
    /// track, or a whole folder). A folder at the edge of its parent leaves it. An error at the
    /// first and last place of the top level. One undo step.
    MoveGroup {
        /// Folder id.
        group: u32,
        /// Down instead of up.
        down: bool,
    },
    /// Set or clear a track's colour (its clips are drawn in it).
    SetTrackColor {
        /// Device.
        device: DeviceKey,
        /// The colour, `None` for the device's own.
        color: Option<[u8; 3]>,
    },
    /// Set a track's comment (blank clears it; otherwise at most
    /// [`crate::organise::MAX_TRACK_COMMENT`] characters, newlines allowed).
    SetTrackComment {
        /// Device.
        device: DeviceKey,
        /// The text.
        comment: String,
    },
    /// Fold or unfold an automation lane in the arrangement (the lane must exist).
    SetLaneFolded {
        /// Device.
        device: DeviceKey,
        /// Param key of the lane.
        param: String,
        /// Fold it to a thin strip.
        folded: bool,
    },
    /// Show or hide the take rows of a take group (the group must exist).
    SetTakesOpen {
        /// Take group id.
        take_group: u32,
        /// Show the rows.
        open: bool,
    },
    /// Add an automation lane for a parameter (creates the track if needed).
    AddAutomationLane {
        /// Device.
        device: DeviceKey,
        /// Param key.
        param: String,
    },
    /// Insert a clip on a lane. `clip.id` is ignored and allocated; the command result reports
    /// the allocated id via `SyncOp::SongChanged`'s companion `Document::last_clip_id()`.
    AddClip {
        /// Device (track).
        device: DeviceKey,
        /// Lane index.
        lane: usize,
        /// Clip (id ignored).
        clip: Clip,
    },
    /// Remove a clip.
    RemoveClip {
        /// Clip id.
        id: ClipId,
    },
    /// Move/resize a clip (start, length, offset, loop length, mute) keeping its content.
    /// `merge` groups drags into one undo step.
    SetClipGeometry {
        /// Clip id.
        id: ClipId,
        /// New start.
        start: i64,
        /// New length.
        length: i64,
        /// New content offset.
        offset: i64,
        /// New loop length.
        loop_length: Option<i64>,
        /// New mute state.
        muted: bool,
        /// Drag-merge group.
        merge: Option<u64>,
    },
    /// Replace a clip's content (notes or automation points) — used by the piano roll and
    /// automation editors and by recording. `merge` groups drags into one undo step.
    SetClipContent {
        /// Clip id.
        id: ClipId,
        /// New content (must match the lane kind).
        content: ClipContent,
        /// Drag-merge group.
        merge: Option<u64>,
    },
    /// Split a clip at a song tick (both halves keep content alignment).
    SplitClip {
        /// Clip id.
        id: ClipId,
        /// Song tick strictly inside the clip.
        at: i64,
    },
    /// Duplicate a clip to a new start on the same lane.
    DuplicateClip {
        /// Clip id.
        id: ClipId,
        /// Start of the copy.
        start: i64,
    },
    /// Replace the tempo map (first event must be at tick 0).
    SetTempoMap(Vec<TempoEvent>),
    /// [`SongCommand::SetTempoMap`] for a drag: commands with the same `merge` id are one undo
    /// step.
    DragTempoMap {
        /// New map.
        map: Vec<TempoEvent>,
        /// Drag-merge group.
        merge: u64,
    },
    /// Set the time signature at bar 1; later meter changes stay at their bars.
    SetMeter {
        /// Beats per bar (1..=32).
        beats_per_bar: u8,
        /// Beat unit (2, 4, 8 or 16).
        beat_unit: u8,
    },
    /// Replace the meter map (first event must be at bar 1, bars strictly increasing).
    SetMeterMap(Vec<MeterEvent>),
    /// [`SongCommand::SetMeterMap`] for a drag: commands with the same `merge` id are one undo
    /// step.
    DragMeterMap {
        /// New map.
        map: Vec<MeterEvent>,
        /// Drag-merge group.
        merge: u64,
    },
    /// Replace the marker list (ids unique, names valid; the model sorts it by `(tick, id)`).
    /// Reports [`crate::SyncOp::ViewChanged`] only: markers never reach the engine.
    SetMarkers(Vec<Marker>),
    /// [`SongCommand::SetMarkers`] for a drag: commands with the same `merge` id are one undo
    /// step.
    DragMarkers {
        /// New list.
        markers: Vec<Marker>,
        /// Drag-merge group.
        merge: u64,
    },
    /// Replace the range list (ids unique, `start < end`; sorted by `(start, id)`). Reports
    /// [`crate::SyncOp::ViewChanged`] only.
    SetRanges(Vec<RangeMarker>),
    /// [`SongCommand::SetRanges`] for a drag: commands with the same `merge` id are one undo
    /// step.
    DragRanges {
        /// New list.
        ranges: Vec<RangeMarker>,
        /// Drag-merge group.
        merge: u64,
    },
    /// Replace the track heights (see [`Song::track_heights`]): every height within
    /// `MIN_TRACK_HEIGHT..=MAX_TRACK_HEIGHT`; entries of devices without a track are dropped.
    /// Reports [`crate::SyncOp::ViewChanged`] only.
    SetTrackHeights(BTreeMap<DeviceKey, u16>),
    /// [`SongCommand::SetTrackHeights`] for a drag: commands with the same `merge` id are one
    /// undo step.
    DragTrackHeights {
        /// New heights.
        heights: BTreeMap<DeviceKey, u16>,
        /// Drag-merge group.
        merge: u64,
    },
    /// Set the loop range and enable flag.
    SetLoop {
        /// Range `[start, end)`, start < end.
        range: (i64, i64),
        /// Enabled.
        enabled: bool,
    },
    /// Set the song end.
    SetSongEnd(i64),
    /// Set or clear a note clip's step grid. The clip must be a note
    /// clip (audio and automation clips take no grid) and the grid must satisfy
    /// [`StepGrid::validate`]. The clip's length and notes do not change. `merge` groups drags
    /// (swing) into one undo step, in the same group as the other clip edits of that clip.
    /// Reports [`crate::SyncOp::SongChanged`].
    SetClipGrid {
        /// Clip id.
        clip: ClipId,
        /// New grid, or `None` to clear it.
        grid: Option<StepGrid>,
        /// Drag-merge group.
        merge: Option<u64>,
    },
    /// Replace a note clip's controller lanes (pitch bend, aftertouch, CC; see
    /// [`crate::controller`]). The clip must be a note clip (an empty list is allowed on any
    /// clip and a no-op there); the lanes are validated: sorted by kind, unique, points sorted by
    /// tick in `0..=MAX_TICK`, values in the kind's range. Lanes without points are dropped.
    /// `merge` groups drags into one undo step, in the same group as the other clip edits.
    /// Reports [`crate::SyncOp::SongChanged`].
    SetClipControllers {
        /// Clip id.
        clip: ClipId,
        /// The clip's whole controller data after the edit.
        lanes: Vec<ControllerLane>,
        /// Drag-merge group.
        merge: Option<u64>,
    },
    /// Set or clear a note clip's scale (what the piano roll highlights and snaps to). The clip
    /// must be a note clip. `None` and a chromatic scale both clear it (a chromatic scale is
    /// stored as no scale). The notes and the engine's view of the clip do not change. One undo
    /// step. Reports [`crate::SyncOp::SongChanged`].
    SetClipScale {
        /// Clip id.
        clip: ClipId,
        /// New scale, or `None` to clear it.
        scale: Option<Scale>,
    },
    /// One undo step that adds an `atomik.audio-track` device (appended to the rack,
    /// key `audio-track`, `audio-track-2`, ...; named `name`, or the descriptor name made
    /// unique), its console channel and master as `AddDevice` does, its track with a single
    /// audio lane, and the cables `interface:in_1/in_2` to `in_l`/`in_r` (a lone `in_1` feeds
    /// both when the document has one input; none without inputs).
    AddAudioTrack {
        /// Device name; blank names are rejected.
        name: Option<String>,
    },
    /// Add an audio clip to the device's audio lane. The asset must be in the pool
    /// (`Command::RegisterAsset`). The new id is reported by `Document::last_clip_id()`. A
    /// recorded take is just this command (later clips win).
    AddAudioClip {
        /// Device (an audio track).
        device: DeviceKey,
        /// Start tick.
        start: i64,
        /// Length in ticks (> 0).
        length: i64,
        /// Asset id.
        asset: String,
        /// First asset frame played.
        source_offset: u64,
        /// Gain in dB.
        gain_db: f32,
        /// Fade-in frames.
        fade_in: u64,
        /// Fade-out frames.
        fade_out: u64,
        /// Fade shape.
        fade_curve: FadeCurve,
    },
    /// Make a take group on a notes or audio lane from stacked takes (a loop recording's laps):
    /// every clip is validated like `AddClip` (ids ignored and allocated), none goes on the lane,
    /// and the group's comp is the takes layered in order (a later take hides an earlier one
    /// where both have content, as stacked clips play), compiled to plain lane clips in the same
    /// undo step. See [`crate::takes`]. Reports [`crate::SyncOp::SongChanged`]; the new group is
    /// the last one with the highest id ([`Song::take_group`]).
    NewTakeGroup {
        /// Device (track).
        device: DeviceKey,
        /// Lane index (a notes or audio lane).
        lane: usize,
        /// The takes, bottom to top (at least one).
        takes: Vec<Clip>,
    },
    /// Make one take play the whole range of its group (the others stay, silent). One undo step.
    PickTake {
        /// Group id.
        group: u32,
        /// Take index.
        take: usize,
    },
    /// Make `[start, end)` play from a take (a comp edit: the range is cut out of whatever played
    /// there, neighbours of the same take join). `start < end` within the group's range. One undo
    /// step.
    SetCompSegment {
        /// Group id.
        group: u32,
        /// Take index.
        take: usize,
        /// First tick.
        start: i64,
        /// End tick (exclusive).
        end: i64,
    },
    /// Delete a take: the ranges it played go to the take below it (above it for the bottom
    /// take). Deleting the last take removes the group and its clips. One undo step.
    DeleteTake {
        /// Group id.
        group: u32,
        /// Take index.
        take: usize,
    },
    /// Dissolve a group: its comp clips stay on the lane as plain clips, the takes are dropped.
    /// One undo step.
    FlattenTakes {
        /// Group id.
        group: u32,
    },
    /// Quantize the notes of these clips (swing, groove and humanize as well) in one undo step:
    /// see [`crate::groove`]. Clips that are not plain note clips (audio, automation, the takes
    /// of a take group) are skipped; an error when no note changes. Reports
    /// [`crate::SyncOp::SongChanged`].
    ApplyQuantize {
        /// The clips, in any order.
        clips: Vec<ClipId>,
        /// What to do.
        quantize: QuantizeParams,
    },
    /// Quantize some notes of one clip (by index in its sorted note list), as `ApplyQuantize`
    /// does; one undo step. Notes out of range are an error.
    QuantizeNotes {
        /// The clip.
        clip: ClipId,
        /// Indices into the clip's notes.
        notes: Vec<usize>,
        /// What to do.
        quantize: QuantizeParams,
    },
    /// Add a groove to the song's library, or replace the one with the same name. One undo step;
    /// reports [`crate::SyncOp::ViewChanged`] (a groove changes nothing until it is applied).
    SetGroove(Groove),
    /// Remove a groove from the song's library by name (not a built-in one). One undo step.
    DeleteGroove {
        /// The groove's name.
        name: String,
    },
    /// Measure the groove of a note clip against the grid of `resolution` and store it in the
    /// library under `name` (replacing a groove of that name): see [`crate::groove::extract_groove`].
    /// One undo step.
    ExtractGroove {
        /// The clip to measure.
        clip: ClipId,
        /// The new groove's name.
        name: String,
        /// The step the slots are measured against.
        resolution: StepResolution,
        /// Slots, `1..=`[`crate::groove::MAX_GROOVE_SLOTS`].
        slots: usize,
    },
    /// Change an audio clip's start, length, mute and content in one undo step (a
    /// trim moves the start and the source offset together). `merge` groups drags, as
    /// `SetClipGeometry` does.
    SetAudioClip {
        /// Clip id.
        id: ClipId,
        /// New start tick.
        start: i64,
        /// New length in ticks.
        length: i64,
        /// New mute state.
        muted: bool,
        /// New content.
        content: AudioClipContent,
        /// Drag-merge group.
        merge: Option<u64>,
    },
    /// Strip silence from an audio clip in one undo step: the clip becomes the given pieces
    /// of itself, each a separate clip, and the parts between them are gone. `pieces` are
    /// frame ranges `[start, end)` from the clip start (see [`crate::strip_silence`] for the
    /// detector), non-empty, ascending, disjoint and inside the clip. The first piece keeps the
    /// clip's id; the others get the next ids (the last is reported by
    /// `Document::last_clip_id()`). Every piece keeps the clip's gain, curve, reverse and
    /// crossfade, takes its slice of the envelope and gets `fade` frames of fade at each cut
    /// edge (a piece that begins or ends where the clip does keeps the clip's own fade there).
    /// Piece edges land on ticks; a piece's source offset follows the tick, so the sound stays
    /// where it was to the frame. Positions in a tempo-ramped song round to a tick.
    StripSilence {
        /// Clip id.
        id: ClipId,
        /// Frame ranges to keep.
        pieces: Vec<(u64, u64)>,
        /// Fade frames at the cut edges.
        fade: u64,
    },
    /// Freezes the track of `device` (one undo step): the instrument and the effects named in
    /// `frozen` are replaced, as far as the engine goes, by the audio in `frozen` (see
    /// [`crate::freeze`]). The track must not be frozen, `device` needs a console channel it is
    /// cabled to, `frozen.effects` must be the first effects of that channel's insert chain, and
    /// the audio must be in the pool and plain (no reverse, no warp). Reports
    /// [`crate::SyncOp::SongChanged`] and [`crate::SyncOp::FrozenChanged`].
    FreezeTrack {
        /// The frozen track's device.
        device: DeviceKey,
        /// The render and what it includes.
        frozen: Frozen,
    },
    /// Unfreezes the track of `device` (one undo step): the track, its clips, its instrument and
    /// its effects are exactly as they were before the freeze. With `keep_audio` the render also
    /// becomes the clip of a new audio track (in the same step) that sits right after the unfrozen
    /// one. The render's asset stays in the
    /// pool while undo can bring it back, and goes at the next save otherwise.
    UnfreezeTrack {
        /// The track's device.
        device: DeviceKey,
        /// Keep the render as a clip on a new audio track.
        keep_audio: bool,
    },
    /// Adds a new audio track with one clip of rendered audio (bounce in place), and mutes the
    /// clips listed in `mute` (one undo step, new track first). The track is wired like
    /// [`SongCommand::AddAudioTrack`]'s.
    AddRenderedTrack {
        /// The name of the new track, or `None` for the default.
        name: Option<String>,
        /// The track the render comes from: the new track goes right after it, in its folder,
        /// with its colour. `None` puts the track at the end of the list.
        after: Option<DeviceKey>,
        /// Song tick where the clip starts.
        start: i64,
        /// Clip length in ticks.
        length: i64,
        /// The rendered audio (a plain clip: no reverse, no warp).
        audio: AudioClipContent,
        /// Clips to mute (the originals), by id.
        mute: Vec<ClipId>,
    },
}

/// An empty song at the given tempo, 4/4, loop bars 1–5 disabled, end at bar 17.
pub fn empty_song(bpm: f64) -> Song {
    let bar = atomik_core::PPQ * 4;
    Song {
        tracks: Vec::new(),
        tempo: vec![TempoEvent::step(0, bpm)],
        meter: vec![MeterEvent {
            bar: 1,
            num: 4,
            den: 4,
        }],
        loop_range: (0, 4 * bar),
        loop_enabled: false,
        end: 16 * bar,
        next_clip_id: 1,
        markers: Vec::new(),
        ranges: Vec::new(),
        track_heights: BTreeMap::new(),
        lane_write: BTreeMap::new(),
        grooves: Vec::new(),
        org: Organisation::default(),
    }
}

mod flatten;

pub use flatten::{
    AudioRegion, Crossfade, audible_spans, crossfades, effective_tracks, flatten_audio,
    flatten_automation, flatten_controllers, flatten_notes,
};

/// Lowest accepted audio clip gain in dB.
pub const AUDIO_GAIN_DB_MIN: f32 = -120.0;
/// Highest accepted audio clip gain in dB.
pub const AUDIO_GAIN_DB_MAX: f32 = 24.0;
/// Largest asset frame count, source offset or fade length the model accepts (2^40 frames,
/// about 250 days at 48 kHz).
pub const MAX_AUDIO_FRAMES: u64 = 1 << 40;
/// The device type of an audio track (built by the engine).
pub const AUDIO_TRACK_TYPE: &str = "atomik.audio-track";

/// Largest tick, clip length, offset or note length the song accepts: 2^40 ticks (about 9 days
/// at 120 BPM). Keeps every sum of two tick values far from `i64` overflow.
pub const MAX_TICK: i64 = 1 << 40;

/// Most repetitions a looped clip may hold (`length <= loop_length * MAX_LOOP_REPEATS`), so
/// flattening stays bounded.
pub const MAX_LOOP_REPEATS: i64 = 4096;

impl Song {
    /// The groove called `name`: one of the song's, else a built-in one.
    pub fn groove(&self, name: &str) -> Option<Groove> {
        self.grooves
            .iter()
            .find(|g| g.name == name)
            .cloned()
            .or_else(|| builtin_groove(name))
    }

    /// The track of `device`, if any.
    pub fn track(&self, device: &DeviceKey) -> Option<&Track> {
        self.tracks.iter().find(|t| &t.device == device)
    }

    /// The write mode of the automation lane of `param` on `device`'s track.
    pub fn write_mode(&self, device: &DeviceKey, param: &str) -> WriteMode {
        self.lane_write
            .get(&(device.clone(), param.to_owned()))
            .copied()
            .unwrap_or_default()
    }

    /// A clip by id, with its track and lane index.
    pub fn clip(&self, id: ClipId) -> Option<(&Track, usize, &Clip)> {
        self.tracks.iter().find_map(|t| {
            t.lanes
                .iter()
                .enumerate()
                .find_map(|(lane, l)| l.clips.iter().find(|c| c.id == id).map(|c| (t, lane, c)))
        })
    }

    /// A take group by id, with its track and lane index.
    pub fn take_group(&self, id: u32) -> Option<(&Track, usize, &TakeGroup)> {
        self.tracks.iter().find_map(|t| {
            t.lanes.iter().enumerate().find_map(|(lane, l)| {
                l.take_groups
                    .iter()
                    .find(|g| g.id == id)
                    .map(|g| (t, lane, g))
            })
        })
    }

    /// Every take group in track, lane and list order, with its device and lane index.
    pub fn take_groups(&self) -> impl Iterator<Item = (&DeviceKey, usize, &TakeGroup)> {
        self.tracks.iter().flat_map(|t| {
            t.lanes
                .iter()
                .enumerate()
                .flat_map(move |(i, l)| l.take_groups.iter().map(move |g| (&t.device, i, g)))
        })
    }

    /// The first free take group id (above every id in use).
    pub fn next_take_group_id(&self) -> u32 {
        self.take_groups()
            .map(|(_, _, g)| g.id)
            .max()
            .map_or(1, |m| m + 1)
    }

    /// The song End in ticks: where the song stops for a render and for the arrangement's
    /// extent. Playback does not stop there. At least 1 (see [`SongCommand::SetSongEnd`]).
    pub fn end_tick(&self) -> i64 {
        self.end
    }

    /// The first free marker id (above every id in use).
    pub fn next_marker_id(&self) -> u32 {
        self.markers.iter().map(|m| m.id).max().map_or(1, |m| m + 1)
    }

    /// The first free range id (above every id in use).
    pub fn next_range_id(&self) -> u32 {
        self.ranges.iter().map(|r| r.id).max().map_or(1, |r| r + 1)
    }

    /// The tick of the nearest marker strictly before `tick`.
    pub fn prev_marker_tick(&self, tick: i64) -> Option<i64> {
        self.markers
            .iter()
            .rev()
            .map(|m| m.tick)
            .find(|&t| t < tick)
    }

    /// The tick of the nearest marker strictly after `tick`.
    pub fn next_marker_tick(&self, tick: i64) -> Option<i64> {
        self.markers.iter().map(|m| m.tick).find(|&t| t > tick)
    }

    /// The tempo map compiled for tick <-> time maths (see [`atomik_core::TempoMap`]). A map
    /// that fails to compile (a document the model never produces) reads as constant at its
    /// first event's tempo, or at 120 BPM.
    pub fn tempo_table(&self) -> TempoTable {
        let points: Vec<TempoPoint> = self
            .tempo
            .iter()
            .map(|e| TempoPoint {
                tick: e.tick,
                bpm: e.bpm,
                ramp: e.ramp,
            })
            .collect();
        TempoTable::new(&points).unwrap_or_else(|_| {
            let bpm = self.tempo.first().map_or(120.0, |e| e.bpm);
            let tempo = Tempo::new(bpm)
                .or_else(|_| Tempo::new(120.0))
                .unwrap_or_else(|_| unreachable!("120 BPM is a valid tempo"));
            TempoTable::constant(tempo)
        })
    }

    /// The meter map compiled for bar <-> tick maths (see [`atomik_core::MeterMap`]); an
    /// invalid map reads as 4/4.
    pub fn meter_table(&self) -> MeterTable {
        MeterTable::new(&self.meter).unwrap_or_else(|_| MeterTable::constant(4, 4))
    }

    /// The song frame of a tick: `round(seconds(tick) * sample_rate)` with the tempo map
    /// (ties up, 0 for negative ticks), from [`atomik_core::TempoMap`] exactly as the engine's
    /// `song_frame` computes it, so the model and the engine agree on every frame position.
    pub fn frame_at(&self, tick: i64, sample_rate: u32) -> u64 {
        self.tempo_table()
            .map()
            .song_frame(tick, f64::from(sample_rate))
    }

    /// The time signature at bar 1 as `(beats_per_bar, beat_unit)`.
    pub fn meter(&self) -> (u8, u8) {
        self.meter.first().map_or((4, 4), |e| (e.num, e.den))
    }

    /// Ticks per bar under the meter at bar 1.
    pub fn bar_ticks(&self) -> i64 {
        bar_ticks(self.meter())
    }

    /// The bar line nearest to `tick` (halves round up; 0 for a negative tick).
    pub fn nearest_bar_line(&self, tick: i64) -> i64 {
        let table = self.meter_table();
        let meter = table.map();
        let start = meter.bar_start(meter.bar_at(tick));
        let next = meter.next_bar_line(tick);
        if tick - start < next - tick {
            start
        } else {
            next
        }
    }

    /// Ticks per bar of the bar that holds `tick`.
    pub fn bar_ticks_at(&self, tick: i64) -> i64 {
        self.meter_table().map().bar_ticks_at(tick)
    }
}

impl Lane {
    /// An empty lane of `kind`.
    pub fn new(kind: LaneKind) -> Self {
        Lane {
            kind,
            name: None,
            muted: false,
            clips: Vec::new(),
            take_groups: Vec::new(),
        }
    }

    /// An empty note lane.
    pub fn notes() -> Self {
        Self::new(LaneKind::Notes)
    }

    /// An empty audio lane.
    pub fn audio() -> Self {
        Self::new(LaneKind::Audio)
    }

    /// An empty automation lane of the parameter with key `param`.
    pub fn automation(param: impl Into<String>) -> Self {
        Self::new(LaneKind::Automation(param.into()))
    }

    /// This lane holding `clips`.
    #[must_use]
    pub fn with_clips(mut self, clips: Vec<Clip>) -> Self {
        self.clips = clips;
        self
    }

    /// This lane holding `take_groups`.
    #[must_use]
    pub fn with_take_groups(mut self, take_groups: Vec<TakeGroup>) -> Self {
        self.take_groups = take_groups;
        self
    }
}

impl Track {
    /// True for an audio track: its first lane is the audio lane. (A track only knows its
    /// device key; [`crate::RackDoc::is_audio_track`] checks the device type too.)
    pub fn is_audio(&self) -> bool {
        self.lanes.first().is_some_and(|l| l.kind.is_audio())
    }

    /// The track's note lanes in order (lane `i` of the iterator is `lanes[i]`).
    pub fn note_lanes(&self) -> impl Iterator<Item = &Lane> {
        self.lanes.iter().take_while(|l| l.kind.is_notes())
    }

    /// How many note lanes the track has (0 for an audio track).
    pub fn note_lane_count(&self) -> usize {
        self.note_lanes().count()
    }

    /// True if lane `i` is a note lane.
    pub fn is_note_lane(&self, i: usize) -> bool {
        i < self.note_lane_count()
    }

    /// The text to show for lane `i`: a note lane's name, else "Notes" (lane 0) or "Notes N"
    /// (the lane with index `N - 1`); an automation lane's param key; "Audio". Empty for a lane
    /// that does not exist.
    pub fn lane_label(&self, i: usize) -> String {
        match self.lanes.get(i) {
            None => String::new(),
            Some(l) => match (&l.kind, &l.name) {
                (_, Some(name)) => name.clone(),
                (LaneKind::Notes, None) if i == 0 => "Notes".into(),
                (LaneKind::Notes, None) => format!("Notes {}", i + 1),
                (LaneKind::Automation(p), None) => p.clone(),
                (LaneKind::Audio, None) => "Audio".into(),
            },
        }
    }

    /// Index of the audio lane, if any.
    pub fn audio_lane(&self) -> Option<usize> {
        self.lanes.iter().position(|l| l.kind.is_audio())
    }

    /// Index of the automation lane of `param`, if any.
    pub fn automation_lane(&self, param: &str) -> Option<usize> {
        self.lanes
            .iter()
            .position(|l| matches!(&l.kind, LaneKind::Automation(p) if p == param))
    }
}

impl Clip {
    /// Song tick where the clip ends (exclusive).
    pub fn end(&self) -> i64 {
        self.start + self.length
    }
}

impl ClipContent {
    /// True for note content.
    pub fn is_notes(&self) -> bool {
        matches!(self, ClipContent::Notes(_))
    }

    /// True for audio content.
    pub fn is_audio(&self) -> bool {
        matches!(self, ClipContent::Audio(_))
    }
}

impl LaneKind {
    /// True for the note lane.
    pub fn is_notes(&self) -> bool {
        matches!(self, LaneKind::Notes)
    }

    /// True for the audio lane.
    pub fn is_audio(&self) -> bool {
        matches!(self, LaneKind::Audio)
    }
}

/// Ticks per beat for a meter (`PPQ * 4 / beat_unit`; at least 1).
pub fn beat_ticks(meter: (u8, u8)) -> i64 {
    (atomik_core::PPQ * 4 / i64::from(meter.1.max(1))).max(1)
}

/// Ticks per bar for a meter (at least 1).
pub fn bar_ticks(meter: (u8, u8)) -> i64 {
    beat_ticks(meter) * i64::from(meter.0.max(1))
}

/// A song tick as a 1-based bar, a 1-based beat and the ticks into that beat, under
/// `meter = (beats_per_bar, beat_unit)`. Negative ticks read as 0; the bar saturates at
/// `u32::MAX`.
pub fn bars_beats_ticks(tick: i64, meter: (u8, u8)) -> (u32, u32, u32) {
    let tick = tick.max(0);
    let (bar, beat) = (bar_ticks(meter), beat_ticks(meter));
    let in_bar = tick % bar;
    let sat = |x: i64| u32::try_from(x).unwrap_or(u32::MAX);
    (
        sat(tick / bar).saturating_add(1),
        sat(in_bar / beat + 1),
        sat(in_bar % beat),
    )
}

/// The inverse of [`bars_beats_ticks`]: `(bar, beat, ticks)` (bar and beat 1-based; 0 reads
/// as 1) to a song tick.
pub fn ticks_from_bars_beats_ticks(bbt: (u32, u32, u32), meter: (u8, u8)) -> i64 {
    let (bar, beat, tick) = bbt;
    i64::from(bar.saturating_sub(1)) * bar_ticks(meter)
        + i64::from(beat.saturating_sub(1)) * beat_ticks(meter)
        + i64::from(tick)
}

/// A song tick as `"bar.beat.tick"` (e.g. `"2.2.5"`), for position readouts.
pub fn format_bbt(tick: i64, meter: (u8, u8)) -> String {
    let (bar, beat, t) = bars_beats_ticks(tick, meter);
    format!("{bar}.{beat}.{t}")
}
