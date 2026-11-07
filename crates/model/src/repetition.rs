//! The repetition finder: ideas a song repeats three times or more, unchanged, in a row, and a
//! few small variations that would renew them.
//!
//! [`find`] is a pure function of the song. It looks at two kinds of idea:
//!
//! * **A bar pattern of notes.** The notes a track plays ([`flatten_track_notes`], so loops, copies
//!   and live grooves are all seen as they sound) are cut at the bar lines. A bar is the notes in
//!   it with their place in the bar, length, key and velocity; two bars are the same only when all
//!   of those are equal. A pattern of 1, 2 or 4 bars that plays [`MIN_REPEATS`] times or more in a
//!   row is an idea. The shortest period that explains a stretch wins (eight identical bars are
//!   one 1-bar idea, not also a 2-bar and a 4-bar one), and a stretch of empty bars is not an idea.
//! * **An audio clip.** Clips of one audio lane that are touching, equally long and identical (the
//!   same part of the same file with the same gain, fades, envelope and warp), or one clip that
//!   loops its content that often.
//!
//! [`variations`] gives an idea its suggestions, three kinds that cost nothing but an edit at the
//! end of the run: drop a hit in the last bar, filter the last bar, cut the last beat. A suggestion
//! that is a simple edit comes with the command that makes it, planned against the document so
//! that it can be run as one undo step; one that cannot be done safely (a looped clip would change
//! every repeat, a lane is already automated, the track has no channel) is advice in text only.

use crate::song::{
    ClipContent, ClipId, Curve, Note, Song, Track, audible_spans, flatten_track_notes,
};
use crate::{Command, DeviceKey, RackDoc, SongCommand};
use atomik_device_sdk::Registry;

/// The fewest repeats of an idea that count.
pub const MIN_REPEATS: u32 = 3;
/// The pattern lengths looked for, in bars, shortest first.
pub const PERIODS: [u32; 3] = [1, 2, 4];
/// The most ideas one search returns.
pub const MAX_IDEAS: usize = 64;
/// The low-pass frequency a filter variation sweeps down to, in Hz.
pub const FILTER_TARGET_HZ: f32 = 1_500.0;

/// What repeats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdeaKind {
    /// A pattern of notes, `bars` bars long.
    NoteBars {
        /// The length of one repeat, in bars.
        bars: u32,
    },
    /// Identical audio clips of one lane, or one looping clip.
    AudioClips {
        /// The lane of the track.
        lane: usize,
        /// The clips of the run in time order (one id for a looping clip).
        clips: Vec<ClipId>,
    },
}

/// An idea repeated [`MIN_REPEATS`] times or more unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Idea {
    /// The track.
    pub track: DeviceKey,
    /// What repeats.
    pub kind: IdeaKind,
    /// The tick where the first repeat starts.
    pub start: i64,
    /// The tick where the last repeat ends.
    pub end: i64,
    /// How many times it plays.
    pub repeats: u32,
}

impl Idea {
    /// The first and last bar (1-based) the run covers.
    pub fn bars(&self, song: &Song) -> (u32, u32) {
        let table = song.meter_table();
        let map = table.map();
        (
            map.bar_at(self.start),
            map.bar_at((self.end - 1).max(self.start)),
        )
    }

    /// The tick range of the last bar of the run (the run's end for a run that stops inside a bar).
    pub fn last_bar(&self, song: &Song) -> (i64, i64) {
        let table = song.meter_table();
        let map = table.map();
        let bar = map.bar_at((self.end - 1).max(self.start));
        (map.bar_start(bar).max(self.start), self.end)
    }

    /// What repeats, in a few words (`a 2-bar pattern`, `an audio clip`).
    pub fn what(&self) -> String {
        match &self.kind {
            IdeaKind::NoteBars { bars: 1 } => "a 1-bar pattern".to_owned(),
            IdeaKind::NoteBars { bars } => format!("a {bars}-bar pattern"),
            IdeaKind::AudioClips { .. } => "an audio clip".to_owned(),
        }
    }
}

/// The signature of a bar: its length in ticks and its notes by place in the bar.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bar {
    ticks: i64,
    notes: Vec<(i64, i64, u8, u32)>,
}

/// The ideas of `song`, in time order (then track order), at most [`MAX_IDEAS`]. Muted tracks are
/// looked at too: a repeat the song carries is one whether it sounds or not.
pub fn find(song: &Song) -> Vec<Idea> {
    let mut out = Vec::new();
    for track in &song.tracks {
        if track.is_automation_only() {
            continue;
        }
        if track.is_audio() {
            audio_ideas(track, &mut out);
        } else {
            note_ideas(song, track, &mut out);
        }
    }
    out.sort_by_key(|i| i.start);
    out.truncate(MAX_IDEAS);
    out
}

fn note_ideas(song: &Song, track: &Track, out: &mut Vec<Idea>) {
    let notes = flatten_track_notes(song, track);
    let (Some(first), Some(last)) = (notes.first(), notes.iter().map(|n| n.0).max()) else {
        return;
    };
    let table = song.meter_table();
    let map = table.map();
    let first_bar = map.bar_at(first.0);
    let bars = (map.bar_at(last) - first_bar + 1) as usize;
    let mut sigs: Vec<Bar> = (0..bars)
        .map(|i| {
            let from = map.bar_start(first_bar + i as u32);
            let to = map.bar_start(first_bar + i as u32 + 1);
            Bar {
                ticks: to - from,
                notes: Vec::new(),
            }
        })
        .collect();
    for &(tick, length, key, velocity) in &notes {
        let i = (map.bar_at(tick) - first_bar) as usize;
        let from = map.bar_start(first_bar + i as u32);
        sigs[i]
            .notes
            .push((tick - from, length, key, velocity.to_bits()));
    }
    let mut claimed = vec![false; bars];
    for period in PERIODS {
        let p = period as usize;
        let mut i = 0;
        while i + p < bars {
            if sigs[i] != sigs[i + p] {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + p < bars && sigs[j] == sigs[j + p] {
                j += 1;
            }
            let repeats = ((j + p - i) / p) as u32;
            let covered = i..i + repeats as usize * p;
            let has_notes = sigs[i..i + p].iter().any(|b| !b.notes.is_empty());
            if repeats >= MIN_REPEATS && has_notes && !covered.clone().any(|b| claimed[b]) {
                covered.clone().for_each(|b| claimed[b] = true);
                out.push(Idea {
                    track: track.device.clone(),
                    kind: IdeaKind::NoteBars { bars: period },
                    start: map.bar_start(first_bar + covered.start as u32),
                    end: map.bar_start(first_bar + covered.end as u32),
                    repeats,
                });
            }
            i = j + 1;
        }
    }
}

fn audio_ideas(track: &Track, out: &mut Vec<Idea>) {
    for (lane_index, lane) in track.lanes.iter().enumerate() {
        if !lane.kind.is_audio() {
            continue;
        }
        let mut clips: Vec<_> = audible_spans(lane)
            .into_iter()
            .filter_map(|(id, ..)| lane.clips.iter().find(|c| c.id == id))
            .filter(|c| matches!(c.content, ClipContent::Audio(_)))
            .collect();
        clips.sort_by_key(|c| (c.start, c.id));
        // A clip that loops its content that often.
        for c in &clips {
            if let Some(n) = c.loop_length.filter(|n| *n > 0) {
                let repeats = (c.length / n) as u32;
                if repeats >= MIN_REPEATS {
                    out.push(Idea {
                        track: track.device.clone(),
                        kind: IdeaKind::AudioClips {
                            lane: lane_index,
                            clips: vec![c.id],
                        },
                        start: c.start,
                        end: c.start + i64::from(repeats) * n,
                        repeats,
                    });
                }
            }
        }
        // Touching, identical, equally long clips.
        let mut i = 0;
        while i < clips.len() {
            let mut j = i + 1;
            while j < clips.len()
                && clips[j - 1].loop_length.is_none()
                && clips[j].loop_length.is_none()
                && clips[j].start == clips[j - 1].end()
                && clips[j].length == clips[i].length
                && clips[j].offset == clips[i].offset
                && clips[j].content == clips[i].content
            {
                j += 1;
            }
            let repeats = (j - i) as u32;
            if repeats >= MIN_REPEATS {
                out.push(Idea {
                    track: track.device.clone(),
                    kind: IdeaKind::AudioClips {
                        lane: lane_index,
                        clips: clips[i..j].iter().map(|c| c.id).collect(),
                    },
                    start: clips[i].start,
                    end: clips[j - 1].end(),
                    repeats,
                });
            }
            i = j;
        }
    }
}

/// The kinds of variation offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariationKind {
    /// Drop the last hit of the last bar.
    DropHit,
    /// A low-pass sweep over the last bar.
    FilterLastBar,
    /// Cut the last beat of the last bar.
    CutBeat,
}

impl VariationKind {
    /// The kinds, in the order they are offered.
    pub const ALL: [VariationKind; 3] = [
        VariationKind::DropHit,
        VariationKind::FilterLastBar,
        VariationKind::CutBeat,
    ];

    /// A short name for a button.
    pub fn label(self) -> &'static str {
        match self {
            VariationKind::DropHit => "Drop a hit",
            VariationKind::FilterLastBar => "Filter the last bar",
            VariationKind::CutBeat => "Cut a beat",
        }
    }
}

/// One suggestion for an idea.
#[derive(Debug, Clone, PartialEq)]
pub struct Variation {
    /// Which suggestion.
    pub kind: VariationKind,
    /// What it does (when there is a command) or what to do by hand (when there is none).
    pub text: String,
    /// The edit that makes it, to run as one undo step; `None` for advice only.
    pub command: Option<Command>,
}

/// The suggestions for `idea`, one per [`VariationKind`], planned against `doc`.
pub fn variations(doc: &RackDoc, registry: &Registry, idea: &Idea) -> Vec<Variation> {
    VariationKind::ALL
        .into_iter()
        .map(|kind| {
            let (text, command) = match kind {
                VariationKind::DropHit => drop_hit(doc, idea),
                VariationKind::FilterLastBar => filter_last_bar(doc, registry, idea),
                VariationKind::CutBeat => cut_beat(doc, idea),
            };
            Variation {
                kind,
                text,
                command,
            }
        })
        .collect()
}

/// A track's notes in the last bar of the run, as `(tick, key)` of what plays.
fn last_bar_notes(doc: &RackDoc, idea: &Idea) -> Vec<(i64, i64, u8, f32)> {
    let Some(track) = doc.song.track(&idea.track) else {
        return Vec::new();
    };
    let (from, to) = idea.last_bar(&doc.song);
    flatten_track_notes(&doc.song, track)
        .into_iter()
        .filter(|n| n.0 >= from && n.0 < to)
        .collect()
}

/// The command that mutes every note of `track` that starts in `[from, to)`, when that is a
/// plain edit of exactly one clip: the notes are in one clip that is not looped, not on a take
/// lane and has no groove or swing of its own, and nothing else plays them. `None` otherwise.
fn mute_range(doc: &RackDoc, track: &Track, range: (i64, i64)) -> Option<Command> {
    let flat = flatten_track_notes(&doc.song, track);
    let playing: usize = flat
        .iter()
        .filter(|n| n.0 >= range.0 && n.0 < range.1)
        .count();
    if playing == 0
        || track.groove.is_some()
        || !track.lanes.iter().all(|l| l.take_groups.is_empty())
    {
        return None;
    }
    let mut found = Vec::new();
    for lane in track.played_note_lanes() {
        for clip in &lane.clips {
            let ClipContent::Notes(notes) = &clip.content else {
                continue;
            };
            if clip.muted
                || clip.loop_length.is_some()
                || clip.grid.is_some()
                || clip.groove.is_some()
            {
                continue;
            }
            let at = |n: &Note| clip.start + (n.tick - clip.offset);
            let hits: Vec<usize> = notes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    let t = at(n);
                    !n.muted && t >= range.0 && t < range.1 && t >= clip.start && t < clip.end()
                })
                .map(|(i, _)| i)
                .collect();
            if !hits.is_empty() {
                found.push((clip, hits));
            }
        }
    }
    let [(clip, hits)] = found.as_slice() else {
        return None;
    };
    if hits.len() != playing {
        return None;
    }
    let ClipContent::Notes(notes) = &clip.content else {
        return None;
    };
    let mut edited = notes.to_vec();
    for &i in hits {
        edited[i].muted = true;
    }
    Some(Command::Song(SongCommand::SetClipContent {
        id: clip.id,
        content: ClipContent::Notes(edited.into()),
        merge: None,
    }))
}

fn drop_hit(doc: &RackDoc, idea: &Idea) -> (String, Option<Command>) {
    let advice = "Mute the last hit of the last bar of the final repeat so the pattern turns \
                  round before it comes back";
    if matches!(idea.kind, IdeaKind::AudioClips { .. }) {
        return (
            "Cut the last hit of the final clip short, or mute its last beat".to_owned(),
            None,
        );
    }
    let notes = last_bar_notes(doc, idea);
    let mut onsets: Vec<i64> = notes.iter().map(|n| n.0).collect();
    onsets.dedup();
    let (Some(last), true) = (onsets.last().copied(), onsets.len() >= 2) else {
        return (
            "The last bar has a single hit: change its length or move it".to_owned(),
            None,
        );
    };
    let command = doc
        .song
        .track(&idea.track)
        .and_then(|t| mute_range(doc, t, (last, last + 1)));
    match command {
        Some(c) => ("Mute the last hit of the last bar".to_owned(), Some(c)),
        None => (advice.to_owned(), None),
    }
}

fn cut_beat(doc: &RackDoc, idea: &Idea) -> (String, Option<Command>) {
    let advice = "Remove the last beat of the last bar of the final repeat";
    if matches!(idea.kind, IdeaKind::AudioClips { .. }) {
        return (advice.to_owned(), None);
    }
    let table = doc.song.meter_table();
    let beat = table.map().beat_ticks_at(idea.last_bar(&doc.song).0);
    let (_, to) = idea.last_bar(&doc.song);
    let from = to - beat;
    let command = doc
        .song
        .track(&idea.track)
        .and_then(|t| mute_range(doc, t, (from, to)));
    match command {
        Some(c) => (
            "Mute the notes of the last beat of the last bar".to_owned(),
            Some(c),
        ),
        None => (advice.to_owned(), None),
    }
}

/// The channel strip that plays `track`.
fn channel_of<'a>(doc: &'a RackDoc, track: &DeviceKey) -> Option<&'a DeviceKey> {
    doc.console
        .channels
        .iter()
        .find(|c| c.source.as_ref() == Some(track))
        .map(|c| &c.channel)
}

fn filter_last_bar(doc: &RackDoc, registry: &Registry, idea: &Idea) -> (String, Option<Command>) {
    let advice = "Sweep a low-pass filter down over the last bar of the final repeat";
    let Some(channel) = channel_of(doc, &idea.track) else {
        return (advice.to_owned(), None);
    };
    let value = |param: &str| doc.param_value(channel, param, registry);
    let (Some(on), Some(freq)) = (value("lpf.on"), value("lpf.freq")) else {
        return (advice.to_owned(), None);
    };
    let existing = doc.song.track(channel);
    let automated = |param: &str| existing.is_some_and(|t| t.automation_lane(param).is_some());
    if on >= 0.5 || automated("lpf.on") || automated("lpf.freq") {
        return (advice.to_owned(), None);
    }
    let (from, to) = idea.last_bar(&doc.song);
    let length = to - from;
    let first_lane = existing.map_or(0, |t| t.lanes.len());
    let clip = |points: Vec<crate::song::AutomationPoint>| crate::song::Clip {
        id: ClipId(0),
        start: from,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        content: ClipContent::Automation(points),
    };
    let point = |tick, value, curve| crate::song::AutomationPoint { tick, value, curve };
    let lane = |param: &str| {
        Command::Song(SongCommand::AddAutomationLane {
            device: channel.clone(),
            param: param.to_owned(),
        })
    };
    let base = |param: &str, value: f32| {
        Command::Song(SongCommand::SetLaneStatic {
            device: channel.clone(),
            param: param.to_owned(),
            value: Some(value),
            merge: None,
        })
    };
    let batch = Command::Batch(vec![
        lane("lpf.on"),
        Command::Song(SongCommand::AddClip {
            device: channel.clone(),
            lane: first_lane,
            clip: clip(vec![point(0, 1.0, Curve::Step)]),
        }),
        base("lpf.on", 0.0),
        lane("lpf.freq"),
        Command::Song(SongCommand::AddClip {
            device: channel.clone(),
            lane: first_lane + 1,
            clip: clip(vec![
                point(0, freq, Curve::Linear),
                point(length, FILTER_TARGET_HZ, Curve::Linear),
            ]),
        }),
        base("lpf.freq", freq),
    ]);
    (
        "Add a low-pass sweep over the last bar on the track's channel".to_owned(),
        Some(batch),
    )
}

#[cfg(test)]
#[path = "repetition_tests.rs"]
mod tests;
