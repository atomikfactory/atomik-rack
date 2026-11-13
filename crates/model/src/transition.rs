//! `WorkflowCommand::AddTransition`: the transitions that are edits, each one undo step, and the
//! helpers the app needs to print the two that are audio.
//!
//! A transition belongs to a **boundary**: the tick where a section starts (a marker, the start of
//! a range, or the section lines of [`crate::energy_map::sections`]). Five kinds exist:
//!
//! * **Riser** and **impact** are printed audio (the app renders them on the freeze worker, see
//!   `atomik_dsp::rumble::reverse_swell` and [`atomik_dsp::transition::impact`]) and land on a new
//!   track with `AddRenderedTrack`. This module says which track holds the next section's first
//!   hit ([`first_hit`]), the span of that hit ([`hit_span`]) and how long a riser of some bars
//!   lasts ([`riser_seconds`]).
//! * **Filter down** ([`TransitionEdit::FilterDown`]) puts an Auto-Filter low-pass as the last
//!   insert of the master or of a group bus, left open, and an automation clip on its `cutoff`
//!   that closes it over the last 1 to 4 bars before the boundary, in steps that follow an
//!   exponential curve. The filter opens again at the boundary (the lane's static value). A second
//!   filter-down on the same strip reuses the filter and its lane.
//! * **Silence** ([`TransitionEdit::Silence`]) brings the master fader to zero for one beat from
//!   the boundary: a clip of one step on the master's `fader` lane, whose static value is the
//!   fader as it was, so the mix comes back after the beat. Beat one is withheld; the next beat
//!   plays at full level.
//! * **Fill** ([`TransitionEdit::Fill`]) takes the last beat before the boundary of a note track's
//!   clips and plays it twice, each copy at double rate: a note on the beat is moved to half of
//!   its distance from the beat's start (and half as long) and again half a beat later. The notes
//!   of that beat are replaced by the two copies; the rest of the clip stays as it is.
//!
//! Every edit is planned on a scratch copy of the document and applied whole or not at all.
//!
//! # Counting
//!
//! A transition that is on most boundaries is no surprise any more. [`overuse`] says when a kind
//! was used on more than half of the boundaries and on at least [`MIN_USES`] of them.

use crate::console_cmd::Planner;
use crate::console_insert::{insert_chain, set_insert};
use crate::history::Edit;
use crate::reverb_send::set;
use crate::rumble::{DRUM_MACHINE_TYPE, FILTER_TYPE};
use crate::song::{
    AutomationPoint, Clip, ClipContent, ClipId, Curve, Note, NoteList, Song, Track,
    flatten_track_notes,
};
use crate::{Command, DeviceKey, InsertEffect, ModelError, RackDoc, SongCommand};
use atomik_device_sdk::Registry;

/// The lane of the master fader.
pub const FADER_LANE: &str = "fader";
/// The lane of the filter's cutoff.
pub const CUTOFF_LANE: &str = "cutoff";
/// The cutoff the filter-down starts from (open), in Hz.
pub const OPEN_HZ: f32 = 20_000.0;
/// The cutoff the filter-down reaches at the boundary, in Hz.
pub const FLOOR_HZ: f32 = 300.0;
/// The bars a filter-down may last.
pub const FILTER_BARS: std::ops::RangeInclusive<u8> = 1..=4;
/// The points of the filter-down curve (one more than its segments).
const CURVE_POINTS: usize = 9;
/// The shortest and longest riser the app prints, in seconds.
pub const RISER_SECONDS: (f32, f32) = (0.2, 20.0);
/// The fewest uses of a kind that count as overuse.
pub const MIN_USES: usize = 3;

/// The kinds of transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransitionKind {
    /// A reverse reverb of the next section's first hit, ending where it starts (printed).
    Riser,
    /// The next section's first hit, saturated, with a long reverb (printed).
    Impact,
    /// A low-pass that closes over the last bars (automation).
    FilterDown,
    /// The master fader at zero for one beat (automation).
    Silence,
    /// The last beat of a note track played twice at double rate (note edits).
    Fill,
}

impl TransitionKind {
    /// Every kind, in menu order.
    pub const ALL: [TransitionKind; 5] = [
        TransitionKind::Riser,
        TransitionKind::Impact,
        TransitionKind::FilterDown,
        TransitionKind::Silence,
        TransitionKind::Fill,
    ];

    /// The name in messages.
    pub fn label(self) -> &'static str {
        match self {
            TransitionKind::Riser => "Riser",
            TransitionKind::Impact => "Impact",
            TransitionKind::FilterDown => "Filter down",
            TransitionKind::Silence => "Silence on beat one",
            TransitionKind::Fill => "Fill",
        }
    }
}

/// The transitions that are edits (the riser and the impact are printed audio).
#[derive(Debug, Clone, PartialEq)]
pub enum TransitionEdit {
    /// Close a low-pass on the master or on a group bus over the last `bars` bars.
    FilterDown {
        /// The master or an output bus.
        target: DeviceKey,
        /// Bars before the boundary, within [`FILTER_BARS`].
        bars: u8,
    },
    /// The master fader at zero for the beat that starts at the boundary.
    Silence,
    /// The last beat of the track's clips, twice at double rate.
    Fill {
        /// The device that has the note track.
        track: DeviceKey,
    },
}

impl TransitionEdit {
    /// The kind of transition it makes.
    pub fn kind(&self) -> TransitionKind {
        match self {
            TransitionEdit::FilterDown { .. } => TransitionKind::FilterDown,
            TransitionEdit::Silence => TransitionKind::Silence,
            TransitionEdit::Fill { .. } => TransitionKind::Fill,
        }
    }
}

fn refuse(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

/// The boundaries of `song`: the starts of its sections after the first, ascending and distinct
/// (markers, range starts, else every few bars; see [`crate::energy_map::sections`]).
pub fn boundaries(song: &Song) -> Vec<i64> {
    let mut ticks: Vec<i64> = crate::energy_map::sections(song)
        .into_iter()
        .map(|s| s.start)
        .chain(song.markers.iter().map(|m| m.tick))
        .chain(song.ranges.iter().map(|r| r.start))
        .filter(|&t| t > 0)
        .collect();
    ticks.sort_unstable();
    ticks.dedup();
    ticks
}

/// The first boundary at or after `tick`, else the last one before it.
pub fn nearest_boundary(song: &Song, tick: i64) -> Option<i64> {
    let all = boundaries(song);
    all.iter()
        .copied()
        .find(|&b| b >= tick)
        .or_else(|| all.last().copied())
}

/// The span `[at, at + one beat)` of the first hit of the section that starts at `at`.
pub fn hit_span(song: &Song, at: i64) -> (i64, i64) {
    let beat = song.meter_table().map().beat_ticks_at(at);
    (at, at + beat)
}

/// The track that plays the first hit of the section that starts at `at`: the one with the
/// earliest note or audio clip start in its first beat, a drum machine first among equals, then
/// the track order. A frozen track and a track that only holds automation are left out. Also the
/// tick of the hit.
pub fn first_hit(doc: &RackDoc, at: i64) -> Option<(DeviceKey, i64)> {
    let (from, to) = hit_span(&doc.song, at);
    let drums = |t: &Track| {
        doc.device(&t.device)
            .is_some_and(|d| d.type_id == DRUM_MACHINE_TYPE)
    };
    doc.song
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.frozen.is_none() && !t.device.0.contains('/'))
        .filter_map(|(i, t)| {
            let onset = if t.is_audio() {
                t.lanes
                    .iter()
                    .flat_map(|l| &l.clips)
                    .filter(|c| !c.muted && (from..to).contains(&c.start))
                    .map(|c| c.start)
                    .min()
            } else {
                flatten_track_notes(&doc.song, t)
                    .into_iter()
                    .map(|(tick, ..)| tick)
                    .filter(|tick| (from..to).contains(tick))
                    .min()
            }?;
            Some((onset, !drums(t), i, t.device.clone()))
        })
        .min_by_key(|(onset, plain, i, _)| (*onset, *plain, *i))
        .map(|(onset, _, _, device)| (device, onset))
}

/// How long a riser of `bars` bars that ends at `at` lasts, in seconds, at the tempo there, kept
/// within [`RISER_SECONDS`]. The bars are those before `at`, as many as the song has.
pub fn riser_seconds(doc: &RackDoc, at: i64, bars: u8) -> f32 {
    let bar = doc.song.bar_ticks_at((at - 1).max(0));
    let from = (at - i64::from(bars) * bar).max(0);
    let clock = doc.song.frame_clock(doc.sample_rate);
    let frames = clock.at(at) - clock.at(from);
    let seconds = frames as f64 / f64::from(doc.sample_rate.max(1));
    (seconds as f32).clamp(RISER_SECONDS.0, RISER_SECONDS.1)
}

/// The group bus the track of `device` plays into: where its channel's output goes, when that
/// is an output bus.
pub fn output_bus(doc: &RackDoc, device: &DeviceKey) -> Option<DeviceKey> {
    let channel = &doc.console.channel_of(device)?.channel;
    match crate::console::output_dest(doc, channel) {
        crate::console::OutputDest::Bus(bus) => Some(bus),
        _ => None,
    }
}

/// The note tracks a fill at `at` can work on: those with a clip that covers the whole beat before
/// it and holds a note in it, a drum machine first.
pub fn fill_tracks(doc: &RackDoc, at: i64) -> Vec<DeviceKey> {
    let mut found: Vec<(bool, usize, DeviceKey)> = doc
        .song
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.frozen.is_none() && !t.device.0.contains('/'))
        .filter(|(_, t)| !fill_clips(&doc.song, t, at).is_empty())
        .map(|(i, t)| {
            let drums = doc
                .device(&t.device)
                .is_some_and(|d| d.type_id == DRUM_MACHINE_TYPE);
            (!drums, i, t.device.clone())
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, _, d)| d).collect()
}

/// The first beat of the window a fill works on, in song ticks: `[at - beat, at)`.
fn fill_window(song: &Song, at: i64) -> (i64, i64) {
    let beat = song.meter_table().map().beat_ticks_at((at - 1).max(0));
    (at - beat, at)
}

/// The clips of `track` a fill at `at` rewrites with their new notes: plain, unmuted note clips
/// of the played lanes that cover the beat before `at` and have a note starting in it.
fn fill_clips(song: &Song, track: &Track, at: i64) -> Vec<(ClipId, Vec<Note>)> {
    let (from, to) = fill_window(song, at);
    if from < 0 {
        return Vec::new();
    }
    let beat = to - from;
    let mut out = Vec::new();
    for clip in track.played_note_lanes().flat_map(|l| &l.clips) {
        let ClipContent::Notes(notes) = &clip.content else {
            continue;
        };
        if clip.muted || clip.loop_length.is_some() || clip.start > from || clip.end() < to {
            continue;
        }
        // The window in the clip's own ticks.
        let (w0, w1) = (
            from - clip.start + clip.offset,
            to - clip.start + clip.offset,
        );
        if !notes.iter().any(|n| (w0..w1).contains(&n.tick)) {
            continue;
        }
        out.push((clip.id, double_rate(notes, (w0, w1), beat)));
    }
    out
}

/// `notes` with the ones that start in `[w0, w1)` replaced by two copies at double rate: moved
/// to half their distance from `w0`, half as long, the second a half beat later.
fn double_rate(notes: &[Note], (w0, w1): (i64, i64), beat: i64) -> Vec<Note> {
    let half = beat / 2;
    let mut out: Vec<Note> = notes
        .iter()
        .filter(|n| !(w0..w1).contains(&n.tick))
        .copied()
        .collect();
    for n in notes.iter().filter(|n| (w0..w1).contains(&n.tick)) {
        let at = (n.tick - w0) / 2;
        for slot in [0, half] {
            let tick = w0 + slot + at;
            let end = w0 + slot + half;
            out.push(Note {
                tick,
                length: (n.length / 2).clamp(1, (end - tick).max(1)),
                ..*n
            });
        }
    }
    out.sort_by_key(|n| (n.tick, n.key, std::cmp::Reverse(n.length)));
    out.dedup_by_key(|n| (n.tick, n.key));
    out
}

fn run(p: &mut Planner<'_>, command: Command) -> Result<(), ModelError> {
    let edit = crate::Document::plan(p.doc(), command, p.registry)?.0;
    p.push(edit);
    Ok(())
}

fn song(command: SongCommand) -> Command {
    Command::Song(command)
}

/// An automation clip on a lane, from `start`, `length` ticks long.
fn automation_clip(start: i64, length: i64, points: Vec<AutomationPoint>) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        name: None,
        color: None,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        content: ClipContent::Automation(points),
    }
}

/// Makes sure `device` has an automation lane for `param` that holds `base` wherever no clip
/// plays, and puts `clip` on it. A lane that exists keeps its static value.
fn automate(
    p: &mut Planner<'_>,
    device: &DeviceKey,
    param: &str,
    base: f32,
    clip: Clip,
) -> Result<(), ModelError> {
    let has_lane = p
        .doc()
        .song
        .track(device)
        .is_some_and(|t| t.automation_lane(param).is_some());
    if !has_lane {
        run(
            p,
            song(SongCommand::AddAutomationLane {
                device: device.clone(),
                param: param.to_owned(),
            }),
        )?;
    }
    let track = p
        .doc()
        .song
        .track(device)
        .ok_or_else(|| refuse(format!("{device} has no track")))?;
    let lane = track
        .automation_lane(param)
        .ok_or_else(|| refuse(format!("{device} has no {param} lane")))?;
    if let Some(busy) = track.lanes[lane]
        .clips
        .iter()
        .find(|c| c.start < clip.end() && c.end() > clip.start)
    {
        return Err(refuse(format!(
            "the {param} lane of {device} has a clip there already (from tick {})",
            busy.start
        )));
    }
    if track.lanes[lane].static_value.is_none() {
        run(
            p,
            song(SongCommand::SetLaneStatic {
                device: device.clone(),
                param: param.to_owned(),
                value: Some(base),
                merge: None,
            }),
        )?;
    }
    run(
        p,
        song(SongCommand::AddClip {
            device: device.clone(),
            lane,
            clip,
        }),
    )
}

fn point(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

/// Plans the transition `edit` at the boundary `at`: one edit that does all of it.
///
/// # Errors
/// `at` is not after the start of the song, a track or strip is missing or frozen, an option is
/// out of its range, there is nothing to do (a fill with no note in the last beat), or a clip,
/// lane, device or cable is refused (a clip that overlaps another on the same lane).
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    at: i64,
    edit: &TransitionEdit,
) -> Result<Edit, ModelError> {
    if at <= 0 {
        return Err(refuse(
            "a transition needs a boundary after the start of the song",
        ));
    }
    let mut p = Planner::new(doc, registry);
    match edit {
        TransitionEdit::FilterDown { target, bars } => filter_down(&mut p, at, target, *bars)?,
        TransitionEdit::Silence => silence(&mut p, at)?,
        TransitionEdit::Fill { track } => fill(&mut p, at, track)?,
    }
    Ok(p.finish())
}

fn silence(p: &mut Planner<'_>, at: i64) -> Result<(), ModelError> {
    let registry = p.registry;
    let master = p
        .doc()
        .console
        .master
        .clone()
        .ok_or_else(|| refuse("the project has no master to silence"))?;
    let (from, to) = hit_span(&p.doc().song, at);
    let base = p
        .doc()
        .param_value(&master, FADER_LANE, registry)
        .ok_or_else(|| refuse("the master has no fader"))?;
    let clip = automation_clip(from, to - from, vec![point(0, 0.0, Curve::Step)]);
    automate(p, &master, FADER_LANE, base, clip)
}

fn filter_down(
    p: &mut Planner<'_>,
    at: i64,
    target: &DeviceKey,
    bars: u8,
) -> Result<(), ModelError> {
    if !FILTER_BARS.contains(&bars) {
        return Err(ModelError::OutOfRange(format!(
            "a filter down lasts {} to {} bars",
            FILTER_BARS.start(),
            FILTER_BARS.end()
        )));
    }
    let registry = p.registry;
    let console = &p.doc().console;
    if console.master.as_ref() != Some(target) && console.bus_index(target).is_none() {
        return Err(refuse(format!("{target} is not the master or a group bus")));
    }
    let bar = p.doc().song.bar_ticks_at((at - 1).max(0));
    let from = (at - i64::from(bars) * bar).max(0);
    let length = at - from;
    // The filter of an earlier filter down, else a new one after the inserts.
    let chain = insert_chain(p.doc(), registry, target);
    let own = chain.iter().find(|k| {
        p.doc().device(k).is_some_and(|d| d.type_id == FILTER_TYPE)
            && p.doc()
                .song
                .track(k)
                .is_some_and(|t| t.automation_lane(CUTOFF_LANE).is_some())
    });
    let filter = match own {
        Some(k) => k.clone(),
        None => {
            set_insert(
                p,
                target,
                chain.len(),
                Some(InsertEffect::New {
                    type_id: FILTER_TYPE.to_owned(),
                }),
            )?;
            let added = insert_chain(p.doc(), registry, target)
                .into_iter()
                .find(|k| !chain.contains(k))
                .ok_or_else(|| refuse("the filter could not be added as an insert"))?;
            // A low-pass at 24 dB per octave, open, and nothing else that moves it.
            for (param, value) in [
                ("mode", 0.0),
                ("slope", 1.0),
                ("cutoff", OPEN_HZ),
                ("resonance", 0.15),
                ("depth", 0.0),
                ("follow", 0.0),
                ("mix", 1.0),
            ] {
                set(p, &added, param, value)?;
            }
            added
        }
    };
    let base = p
        .doc()
        .param_value(&filter, CUTOFF_LANE, registry)
        .unwrap_or(OPEN_HZ);
    let segments = (CURVE_POINTS - 1) as i64;
    let points = (0..CURVE_POINTS as i64)
        .map(|i| {
            let share = i as f32 / segments as f32;
            // Exponential in Hz: the same share of the way down in every octave.
            let hz = OPEN_HZ * (FLOOR_HZ / OPEN_HZ).powf(share);
            point(length * i / segments, hz, Curve::Linear)
        })
        .collect();
    let clip = automation_clip(from, length, points);
    automate(p, &filter, CUTOFF_LANE, base, clip)
}

fn fill(p: &mut Planner<'_>, at: i64, track: &DeviceKey) -> Result<(), ModelError> {
    let (from, _) = fill_window(&p.doc().song, at);
    if from < 0 {
        return Err(refuse("there is no beat before the boundary to fill"));
    }
    let t = p
        .doc()
        .song
        .track(track)
        .ok_or_else(|| refuse(format!("{track} has no track")))?;
    let clips = fill_clips(&p.doc().song, t, at);
    if clips.is_empty() {
        return Err(refuse(format!(
            "{track} has no note clip that plays through the last beat before the boundary"
        )));
    }
    for (id, notes) in clips {
        run(
            p,
            song(SongCommand::SetClipContent {
                id,
                content: ClipContent::Notes(NoteList::from(notes)),
                merge: None,
            }),
        )?;
    }
    Ok(())
}

/// A kind that was used on most boundaries (see [`overuse`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overuse {
    /// The kind.
    pub kind: TransitionKind,
    /// The boundaries it is on.
    pub used: usize,
    /// All the boundaries.
    pub of: usize,
}

impl Overuse {
    /// The warning in the status line.
    pub fn message(&self) -> String {
        format!(
            "{} is on {} of {} boundaries: a transition that is everywhere stops being a surprise",
            self.kind.label(),
            self.used,
            self.of
        )
    }
}

/// Whether `kind` is on more than half of the boundaries of `song` and on at least [`MIN_USES`]
/// of them, given the transitions made so far as `(boundary, kind)`. A boundary that has a
/// transition but is not a section boundary counts as one.
pub fn overuse(
    song: &Song,
    uses: &[(i64, TransitionKind)],
    kind: TransitionKind,
) -> Option<Overuse> {
    let mut used: Vec<i64> = uses
        .iter()
        .filter(|(_, k)| *k == kind)
        .map(|(t, _)| *t)
        .collect();
    used.sort_unstable();
    used.dedup();
    let mut all = boundaries(song);
    all.extend(used.iter().copied());
    all.sort_unstable();
    all.dedup();
    (used.len() >= MIN_USES && used.len() * 2 > all.len()).then_some(Overuse {
        kind,
        used: used.len(),
        of: all.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(tick: i64, length: i64, key: u8) -> Note {
        Note::new(tick, length, key, 0.8)
    }

    #[test]
    fn the_last_beat_is_played_twice_at_double_rate() {
        let beat = 960;
        let notes = vec![
            note(0, 100, 36),
            note(960, 480, 36),
            note(1_200, 240, 38),
            note(1_680, 100, 42),
            note(1_920, 100, 36),
        ];
        let out = double_rate(&notes, (960, 1_920), beat);
        let ticks: Vec<(i64, u8)> = out.iter().map(|n| (n.tick, n.key)).collect();
        assert_eq!(
            ticks,
            vec![
                (0, 36),
                (960, 36),
                (1_080, 38),
                (1_320, 42),
                (1_440, 36),
                (1_560, 38),
                (1_800, 42),
                (1_920, 36),
            ]
        );
        // Half as long, never into the next copy.
        assert!(out.iter().all(|n| n.length >= 1));
        assert_eq!(out[1].length, 240);
    }

    #[test]
    fn a_copy_never_runs_past_its_half_beat() {
        let out = double_rate(&[note(0, 5_000, 36)], (0, 960), 960);
        assert!(out.iter().all(|n| n.tick + n.length <= 960));
    }
}
