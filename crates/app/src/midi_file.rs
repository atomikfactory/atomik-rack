//! MIDI files and the song: the conversion between a Standard MIDI File
//! ([`atomik_midi_io::smf`]) and the model's notes, controller lanes, tempo map, meter map and
//! markers. No file access and no document here; the app core reads and writes the files and
//! applies the result as commands.
//!
//! **Export** writes what plays, not the raw clips: a clip, a track or the song is flattened
//! with the model's own rules (`flatten_track_notes`, `flatten_controllers`: loops, offsets, swing, a
//! track's live groove, a later clip hiding an earlier one, muted notes and clips left out), so the file sounds like
//! the arrangement.
//!
//! - The resolution is a choice ([`MidiResolution`]): the native [`PPQ`](atomik_core::PPQ) (15 360
//!   ticks per quarter note, a valid 15-bit division), so every model tick survives a round trip
//!   unchanged, or 960 or 480 for hardware that dislikes a division that large. At a coarser
//!   resolution every tick is rounded to the nearest file tick (a note keeps at least one tick,
//!   a note that a following note of the same key leaves no tick for is dropped). Velocities are
//!   7 bit (`round(v * 127)`, at least 1), pitch bend 14 bit, other controllers 7 bit.
//! - A clip or a track is a type 0 file (one `MTrk`, channel 1); the song is a type 1 file: the
//!   first `MTrk` holds the song name, tempo map, time signatures and markers, then one `MTrk`
//!   per note track (named after its device, channel `index mod 16`). Audio and automation
//!   lanes are not exported.
//! - Tempo ramps become a staircase (a tempo event every sixteenth note, at most 2048 per
//!   ramp); the time signature events sit on the bar lines; point markers are marker meta
//!   events, and so are range markers: one named after the range at its start and one named
//!   `<name> (end)` at its end. A clip file carries the tempo and time signature in effect where
//!   the clip starts, a track file the whole tempo map, meter map and markers.
//! - Program changes a clip carries are written on the track's channel, after the controllers of
//!   their tick, each preceded by the bank select (`CC 0` MSB, `CC 32` LSB) that belongs to it
//!   (a bank select an older project keeps in a controller lane is still written with the lane),
//!   and so are the SysEx messages it carries (see `Clip::midi`).
//! - A linear controller segment is sampled (every 1/16 quarter note, only the changes); a step
//!   point is one event. Notes of one key that overlap are shortened so every note-off pairs with
//!   its own note-on.
//!
//! **Import** ([`read_song`]) turns every `MTrk` into one [`MidiTrack`] per channel that has
//! notes (named after the track, `Track n`, with ` ch n` when a track uses several channels),
//! with the model's rules: a note-on with velocity 0 is a note-off, a note-on on a sounding key
//! ends the earlier note there (and replaces it when both start at one tick), a hanging note
//! ends with its track, a note is at least one tick long, two notes at the same tick and key keep
//! the first. Controllers become `Step` points
//! (the last value at a tick wins); channel-mode controllers (120 and above), data entry and
//! (N)RPN numbers are skipped. Program changes (the last one at a tick wins) are kept in the
//! clip's `midi` data (at most as many as a clip holds): a bank select (`CC 0`, `CC 32`) just
//! before a program change at the same tick joins it as one bank and program event and leaves
//! the controller lanes. The complete SysEx messages of a file track belong to its first clip; a
//! track with SysEx but no notes becomes a clip without notes that carries them. A channel that
//! has controllers and no notes is not a clip: its lanes are the track's own
//! (`MidiTrack::track_controllers`; they go to the first track of the same file track that has
//! notes, else to a track of their own without a clip), and a channel with only program changes
//! is left out. Tempo, time
//! signature and marker events from every track make the song-level data; a marker named
//! `<name> (end)` after a marker named `<name>` closes that marker into a range, which is how
//! the export writes a range. Ticks are rescaled from the file's division with rounding.

use atomik_core::mpe::MEMBER_BEND_RANGE;
use atomik_core::{ChannelRole, MpeZones, PPQ};
use atomik_midi_io::smf::{SmfError, SmfEvent, SmfFile, SmfMessage, SmfTrack, read};
use atomik_model::midi_data::{MAX_PROGRAM_CHANGES, MAX_SYSEX_BYTES, MAX_SYSEX_EVENTS};
use atomik_model::note_expression::MAX_EXPRESSION_POINTS;
use atomik_model::song::{
    AutomationPoint, Clip, ClipContent, ClipId, Curve, FlatExpression, Lane, MAX_MARKER_NAME,
    MAX_TICK, MeterEvent, Note, Song, TempoEvent, Track, bar_ticks, flatten_controllers,
    flatten_expression, flatten_midi_data, flatten_notes_with, flatten_track_notes,
};
use atomik_model::theory::ScaleKind;
use atomik_model::{
    ChordEvent, ChordSymbol, ControllerKind, ControllerLane, DeviceKey, ExpressionKind,
    ExpressionPoint, Key, MidiData, NoteExpression, ProgramChange, SysexEvent, parse_chord,
};
use std::collections::BTreeMap;
use std::path::Path;

/// The most markers an import takes from one file (the rest is dropped).
pub const MAX_IMPORTED_MARKERS: usize = 1000;

/// File extensions the import dialog offers: the list the file browser classifies by.
pub use atomik_library::MIDI_EXTENSIONS;

/// True if `path` has one of the [`MIDI_EXTENSIONS`] (any case).
pub fn is_midi_path(path: &Path) -> bool {
    atomik_library::classify(path, false) == Some(atomik_library::FileKind::Midi)
}

/// Why a MIDI file could not be imported.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MidiFileError {
    /// The file is not a MIDI file this reader handles.
    #[error(transparent)]
    Smf(#[from] SmfError),
    /// The file holds no notes, controllers or SysEx (only tempo, markers or program changes).
    #[error("the MIDI file has no notes")]
    NoNotes,
}

/// The ticks per quarter note a MIDI file is written at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MidiResolution {
    /// The model's own resolution ([`PPQ`]): no tick is rounded.
    #[default]
    Native,
    /// 960 ticks per quarter note.
    Ppq960,
    /// 480 ticks per quarter note.
    Ppq480,
}

impl MidiResolution {
    /// Every choice, finest first.
    pub const ALL: [MidiResolution; 3] = [
        MidiResolution::Native,
        MidiResolution::Ppq960,
        MidiResolution::Ppq480,
    ];

    /// The file's division.
    pub fn ppq(self) -> u16 {
        match self {
            MidiResolution::Native => u16::try_from(PPQ).unwrap_or(u16::MAX),
            MidiResolution::Ppq960 => 960,
            MidiResolution::Ppq480 => 480,
        }
    }

    /// The id used in command arguments and `workflow.json`.
    pub fn id(self) -> &'static str {
        match self {
            MidiResolution::Native => "native",
            MidiResolution::Ppq960 => "960",
            MidiResolution::Ppq480 => "480",
        }
    }

    /// The choice with this id.
    pub fn from_id(id: &str) -> Option<MidiResolution> {
        MidiResolution::ALL.into_iter().find(|r| r.id() == id)
    }

    /// Model ticks per file tick.
    fn divisor(self) -> i64 {
        PPQ / i64::from(self.ppq())
    }

    /// `tick` (model ticks, not negative) in file ticks, rounded to the nearest.
    fn scale(self, tick: i64) -> i64 {
        let d = self.divisor();
        (tick.max(0) + d / 2) / d
    }
}

/// A velocity as 7 bit, never 0 (a note-on with velocity 0 is a note-off).
fn velocity7(v: f32) -> u8 {
    (v * 127.0).round().clamp(1.0, 127.0) as u8
}

fn level7(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 127.0).round() as u8
}

fn bend14(v: f32) -> u16 {
    ((v.clamp(-1.0, 1.0) * 8192.0).round() as i32 + 8192).clamp(0, 16383) as u16
}

fn micros_of(bpm: f64) -> u32 {
    (60_000_000.0 / bpm).round().clamp(1.0, 16_777_215.0) as u32
}

fn bpm_of(micros: u32) -> f64 {
    ((60_000_000.0 / f64::from(micros)) * 1000.0).round() / 1000.0
}

// ------------------------------------------------------------------------------------ export

/// A controller kind's flattened points `(tick, value, curve)`.
type FlatController = (ControllerKind, Vec<(i64, f32, Curve)>);

/// A message with the order it takes among the messages of one tick (names first, then the
/// song-level events, note-offs, controllers, note-ons).
struct Timed {
    tick: i64,
    rank: u8,
    message: SmfMessage,
}

fn timed(tick: i64, rank: u8, message: SmfMessage) -> Timed {
    Timed {
        tick,
        rank,
        message,
    }
}

/// The tempo events of the song: a step, or a ramp sampled as a staircase.
fn tempo_events(tempo: &[TempoEvent]) -> Vec<Timed> {
    let mut out = Vec::new();
    for (i, e) in tempo.iter().enumerate() {
        out.push(timed(e.tick, 1, SmfMessage::Tempo(micros_of(e.bpm))));
        let Some(next) = tempo.get(i + 1).filter(|_| e.ramp) else {
            continue;
        };
        let span = next.tick - e.tick;
        let step = (PPQ / 4).max((span + 2047) / 2048);
        let mut last = micros_of(e.bpm);
        let mut t = e.tick + step;
        while t < next.tick {
            let bpm = e.bpm + (next.bpm - e.bpm) * (t - e.tick) as f64 / span as f64;
            let micros = micros_of(bpm);
            if micros != last {
                out.push(timed(t, 1, SmfMessage::Tempo(micros)));
                last = micros;
            }
            t += step;
        }
    }
    out
}

fn time_signature(num: u8, den: u8) -> SmfMessage {
    SmfMessage::TimeSignature {
        numerator: num,
        denominator_power: den.max(1).trailing_zeros() as u8,
    }
}

/// Tempo map, time signatures (on the bar lines), point markers and range markers of the song.
fn conductor_events(song: &Song) -> Vec<Timed> {
    let mut out = tempo_events(&song.tempo);
    let table = song.meter_table();
    for s in table.segments() {
        out.push(timed(s.tick, 1, time_signature(s.num, s.den)));
    }
    for m in &song.markers {
        out.push(timed(m.tick, 1, SmfMessage::Marker(m.name.clone())));
    }
    for r in &song.ranges {
        out.push(timed(r.start, 1, SmfMessage::Marker(r.name.clone())));
        out.push(timed(
            r.end,
            1,
            SmfMessage::Marker(format!("{} (end)", r.name)),
        ));
    }
    out
}

/// Note-on and note-off events of flattened notes `(tick, length, key, velocity)` on `channel`,
/// at `res` (each tick is a whole file tick; see the module docs for notes that get short).
fn note_events(notes: &[(i64, i64, u8, f32)], channel: u8, res: MidiResolution) -> Vec<Timed> {
    note_events_on(notes, &vec![channel; notes.len()], res)
}

/// [`note_events`] with a channel of its own for each note (`channels[i]` for `notes[i]`).
fn note_events_on(
    notes: &[(i64, i64, u8, f32)],
    channels: &[u8],
    res: MidiResolution,
) -> Vec<Timed> {
    let d = res.divisor();
    let starts: Vec<i64> = notes.iter().map(|n| res.scale(n.0)).collect();
    // Overlapping notes of one key are cut at the next one's start.
    let mut ends: Vec<i64> = notes.iter().map(|n| res.scale(n.0 + n.1.max(1))).collect();
    let mut next_of_key: BTreeMap<u8, usize> = BTreeMap::new();
    let mut next_start: Vec<Option<i64>> = vec![None; notes.len()];
    for i in (0..notes.len()).rev() {
        if let Some(&j) = next_of_key.get(&notes[i].2) {
            ends[i] = ends[i].min(starts[j]);
            next_start[i] = Some(starts[j]);
        }
        next_of_key.insert(notes[i].2, i);
    }
    let mut out = Vec::new();
    for (i, n) in notes.iter().enumerate() {
        let (start, mut end) = (starts[i], ends[i]);
        if end <= start {
            // Shorter than a file tick: it keeps one tick unless the next note of the key
            // starts in this very tick (at the native resolution that note still follows).
            let crowded = next_start[i].is_some_and(|s| s <= start);
            if crowded && res != MidiResolution::Native {
                continue;
            }
            end = start + 1;
        }
        out.push(timed(
            start * d,
            4,
            SmfMessage::NoteOn {
                channel: channels[i],
                key: n.2,
                velocity: velocity7(n.3),
            },
        ));
        out.push(timed(
            end * d,
            2,
            SmfMessage::NoteOff {
                channel: channels[i],
                key: n.2,
                velocity: 0,
            },
        ));
    }
    out
}

/// The semitones a member channel's pitch bend spans each way in an exported MPE track (the MPE
/// default, announced with RPN 0 on every member channel used).
const MPE_BEND_RANGE: f32 = MEMBER_BEND_RANGE;

/// The notes of an MPE track and their expression as events: an MPE Configuration Message (RPN 6
/// on channel 1) and the bend range (RPN 0) of each member channel used at the start, every note
/// on a member channel that is free when it starts (the one free longest; when none is free and
/// all 15 are used, the one whose note ends first), and the curves of a note as pitch bend (the
/// semitones over 48, 14 bit), channel pressure and CC 74 on its channel: the value at the start
/// just before the note on, then each point, then the channel back to rest after the note off.
fn mpe_events(
    notes: &[(i64, i64, u8, f32)],
    expression: &[FlatExpression],
    res: MidiResolution,
) -> Vec<Timed> {
    // Channels, assigned in order of the starts (the notes are sorted by tick and key).
    let mut busy_until: Vec<i64> = Vec::new();
    let mut channels = Vec::with_capacity(notes.len());
    for n in notes {
        let free = (0..busy_until.len())
            .filter(|&c| busy_until[c] <= n.0)
            .min_by_key(|&c| busy_until[c]);
        let c = match free {
            Some(c) => c,
            None if busy_until.len() < usize::from(atomik_core::mpe::MAX_MEMBERS) => {
                busy_until.push(0);
                busy_until.len() - 1
            }
            None => (0..busy_until.len())
                .min_by_key(|&c| busy_until[c])
                .unwrap_or(0),
        };
        busy_until[c] = n.0 + n.1.max(1);
        channels.push(1 + c as u8);
    }
    let members = busy_until.len().max(1) as u8;
    let mut out = Vec::new();
    let controller = |channel: u8, controller: u8, value: u8| SmfMessage::Controller {
        channel,
        controller,
        value,
    };
    // The announcement: the zone, then the bend range of each member channel used.
    for (c, v) in [(101, 0), (100, 6), (6, members)] {
        out.push(timed(0, 1, controller(0, c, v)));
    }
    for ch in 1..=members {
        for (c, v) in [
            (101, 0),
            (100, 0),
            (6, MPE_BEND_RANGE as u8),
            (38, 0),
            (101, 127),
            (100, 127),
        ] {
            out.push(timed(0, 1, controller(ch, c, v)));
        }
    }
    out.extend(note_events_on(notes, &channels, res));
    let d = res.divisor();
    for (i, n) in notes.iter().enumerate() {
        let Some(x) = expression.iter().find(|x| (x.tick, x.key) == (n.0, n.2)) else {
            continue;
        };
        let channel = channels[i];
        let start = res.scale(n.0);
        let end = res.scale(n.0 + n.1.max(1)).max(start + 1);
        type Make = fn(u8, f32) -> SmfMessage;
        let curves: [(&[(i64, f32)], Make); 3] = [
            (&x.pitch, |channel, v| SmfMessage::PitchBend {
                channel,
                value: bend14(v / MPE_BEND_RANGE),
            }),
            (&x.pressure, |channel, v| SmfMessage::ChannelPressure {
                channel,
                pressure: level7(v),
            }),
            (&x.timbre, |channel, v| SmfMessage::Controller {
                channel,
                controller: 74,
                value: level7(v),
            }),
        ];
        for (points, make) in curves {
            let Some(first) = points.first() else {
                continue;
            };
            // The value a note starts with, sent before its note on.
            out.push(timed(start * d, 3, make(channel, first.1)));
            let mut last = make(channel, first.1);
            for &(tick, value) in points.iter().filter(|p| p.0 > n.0) {
                let at = res.scale(tick);
                if at >= end {
                    break;
                }
                let message = make(channel, value);
                if message != last {
                    out.push(timed(at * d, 3, message.clone()));
                    last = message;
                }
            }
            // Back to rest once the note has ended, for the next note on the channel.
            out.push(timed(end * d, 3, make(channel, 0.0)));
        }
    }
    out
}

fn controller_message(kind: ControllerKind, value: f32, channel: u8) -> SmfMessage {
    match kind {
        ControllerKind::PitchBend => SmfMessage::PitchBend {
            channel,
            value: bend14(value),
        },
        ControllerKind::ChannelPressure => SmfMessage::ChannelPressure {
            channel,
            pressure: level7(value),
        },
        ControllerKind::Cc(controller) => SmfMessage::Controller {
            channel,
            controller,
            value: level7(value),
        },
        ControllerKind::PolyPressure(key) => SmfMessage::PolyPressure {
            channel,
            key,
            pressure: level7(value),
        },
    }
}

/// Controller events: a point is an event, a linear segment is sampled between its points.
fn controller_events(lanes: &[FlatController], channel: u8) -> Vec<Timed> {
    let mut out = Vec::new();
    for (kind, points) in lanes {
        for (i, &(tick, value, curve)) in points.iter().enumerate() {
            let message = controller_message(*kind, value, channel);
            let mut last = message.clone();
            out.push(timed(tick, 3, message));
            let Some(&(next_tick, next_value, _)) =
                points.get(i + 1).filter(|_| curve == Curve::Linear)
            else {
                continue;
            };
            let span = next_tick - tick;
            let step = (PPQ / 16).max((span + 4095) / 4096);
            let mut t = tick + step;
            while t < next_tick {
                let v = value + (next_value - value) * (t - tick) as f32 / span as f32;
                let message = controller_message(*kind, v, channel);
                if message != last {
                    out.push(timed(t, 3, message.clone()));
                    last = message;
                }
                t += step;
            }
        }
    }
    out
}

/// Sorts the messages and turns them into a track ending at `end` (at least the last tick), its
/// ticks at `res`.
fn smf_track(mut events: Vec<Timed>, end: i64, res: MidiResolution) -> SmfTrack {
    events.sort_by_key(|e| (e.tick, e.rank));
    let tick = |t: i64| res.scale(t) as u64;
    let end_tick = events.last().map_or(0, |e| tick(e.tick)).max(tick(end));
    SmfTrack {
        events: events
            .into_iter()
            .map(|e| SmfEvent {
                tick: tick(e.tick),
                message: e.message,
            })
            .collect(),
        end_tick,
    }
}

fn named(name: &str) -> Timed {
    timed(0, 0, SmfMessage::TrackName(name.to_owned()))
}

/// The notes, controllers, program changes and SysEx messages a track plays, as events on
/// `channel` (see the module docs).
fn play_events(track: &Track, channel: u8, song: Option<&Song>, res: MidiResolution) -> Vec<Timed> {
    let notes = match song {
        Some(song) => flatten_track_notes(song, track),
        None => flatten_notes_with(track, false),
    };
    let expression = match song {
        Some(song) => flatten_expression(song, track),
        None => flatten_expression(&atomik_model::empty_song(120.0), track),
    };
    // A track with per-note expression is written as MPE: its notes on the member channels of a
    // lower zone, everything else on the zone's master channel (channel 1).
    let (mut out, channel) = if expression.is_empty() {
        (note_events(&notes, channel, res), channel)
    } else {
        (mpe_events(&notes, &expression, res), 0)
    };
    out.extend(controller_events(&flatten_controllers(track), channel));
    let extra = flatten_midi_data(track);
    for p in extra.programs {
        // The bank select goes out just before the program change it belongs to.
        for (controller, value) in [(0, p.bank_msb), (32, p.bank_lsb)] {
            if let Some(value) = value {
                out.push(timed(
                    p.tick,
                    3,
                    SmfMessage::Controller {
                        channel,
                        controller,
                        value,
                    },
                ));
            }
        }
        out.push(timed(
            p.tick,
            3,
            SmfMessage::Program {
                channel,
                program: p.program,
            },
        ));
    }
    out.extend(
        extra
            .sysex
            .into_iter()
            .map(|(tick, data)| timed(tick, 3, SmfMessage::SysEx(data))),
    );
    out
}

fn file(format: u16, tracks: Vec<SmfTrack>, res: MidiResolution) -> SmfFile {
    SmfFile {
        format,
        ppq: res.ppq(),
        tracks,
    }
}

/// A one-lane track holding `clip` alone, started at tick 0 and unmuted: what a clip file plays.
fn lone_clip_track(clip: &Clip) -> Track {
    Track {
        device: DeviceKey(String::new()),
        muted: false,
        solo: false,
        armed: false,
        chase: true,
        notes_to: None,
        direct_record: false,
        controllers: Vec::new(),
        groove: None,
        frozen: None,
        lanes: vec![Lane::notes().with_clips(vec![Clip {
            start: 0,
            muted: false,
            ..clip.clone()
        }])],
    }
}

/// A note clip as a type 0 file: the clip's notes and controllers from tick 0 for the clip's
/// length, with the tempo and time signature in effect where the clip starts.
pub fn clip_file(song: &Song, clip: &Clip, name: &str, res: MidiResolution) -> Vec<u8> {
    let table = song.tempo_table();
    let meter = song.meter_table();
    let bpm = table.map().bpm(clip.start as f64);
    let (num, den) = meter.map().meter_at(clip.start);
    let mut events = vec![
        named(name),
        timed(0, 1, SmfMessage::Tempo(micros_of(bpm))),
        timed(0, 1, time_signature(num, den)),
    ];
    events.extend(play_events(&lone_clip_track(clip), 0, None, res));
    file(0, vec![smf_track(events, clip.length, res)], res).to_bytes()
}

/// A track as a type 0 file: its notes and controllers at their song ticks, with the song's
/// tempo map, time signatures and markers.
pub fn track_file(song: &Song, track: &Track, name: &str, res: MidiResolution) -> Vec<u8> {
    let mut events = vec![named(name)];
    events.extend(conductor_events(song));
    events.extend(play_events(track, 0, Some(song), res));
    file(0, vec![smf_track(events, 0, res)], res).to_bytes()
}

/// The song as a type 1 file: a conductor track (name, tempo map, time signatures, markers) and
/// one track per note track, named by `name_of` (see the module docs).
pub fn song_file(
    song: &Song,
    name: &str,
    name_of: &dyn Fn(&DeviceKey) -> String,
    res: MidiResolution,
) -> Vec<u8> {
    let mut events = vec![named(name)];
    events.extend(conductor_events(song));
    let mut tracks = vec![smf_track(events, 0, res)];
    let note_tracks = song
        .tracks
        .iter()
        .filter(|t| t.lanes.first().is_some_and(|l| l.kind.is_notes()));
    for (i, track) in note_tracks.enumerate() {
        let channel = (i % 16) as u8;
        let mut events = vec![named(&name_of(&track.device))];
        events.extend(play_events(track, channel, Some(song), res));
        tracks.push(smf_track(events, 0, res));
    }
    file(1, tracks, res).to_bytes()
}

// ------------------------------------------------------------------------------------ import

/// One imported track: a MIDI track (and channel) that has notes, in model ticks from the start
/// of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiTrack {
    /// The track's name (see the module docs).
    pub name: String,
    /// The notes, sorted by `(tick, key)`, unique, lengths above 0.
    pub notes: Vec<Note>,
    /// The controller lanes, canonical (sorted by kind, no empty lane).
    pub controllers: Vec<ControllerLane>,
    /// The program changes and SysEx messages of the track, canonical and within the limits of
    /// a clip.
    pub midi: MidiData,
    /// The controller lanes of the channels of this file track that have controllers and no
    /// notes: they belong to the track itself, not to its clip (in model ticks from the start of
    /// the file, canonical). They go to the first track of the file track that has notes, else
    /// to a track of their own.
    pub track_controllers: Vec<ControllerLane>,
    /// The tick the track ended at (its end-of-track event, or its last event).
    pub end: i64,
}

/// What a MIDI file holds, in model terms.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiSong {
    /// One entry per MIDI track and channel with notes, in file order.
    pub tracks: Vec<MidiTrack>,
    /// The tempo map (first event at tick 0), `None` for a file without tempo events.
    pub tempo: Option<Vec<TempoEvent>>,
    /// The meter map (first event at bar 1), `None` for a file without time signatures.
    pub meter: Option<Vec<MeterEvent>>,
    /// The markers as `(tick, name)`, without the ones that make up a range.
    pub markers: Vec<(i64, String)>,
    /// The ranges as `(start, end, name)`: a marker pair as the export writes it.
    pub ranges: Vec<ImportedRange>,
    /// How many channels with only program changes (no notes, no controllers) were left out.
    pub skipped: usize,
    /// The chords of the text, lyric and marker events that read as chord symbols (`Am7`,
    /// `C/G`), each up to the next one (the last one to the end of the file), sorted.
    pub chords: Vec<ChordEvent>,
    /// The key of the first key signature event.
    pub key: Option<Key>,
}

/// A range read from a file: `(start, end, name)` in model ticks.
pub type ImportedRange = (i64, i64, String);

/// A tick of the file in model ticks (rounded, at most [`MAX_TICK`]).
fn model_tick(tick: u64, ppq: u16) -> i64 {
    let scaled = (u128::from(tick) * PPQ as u128 + u128::from(ppq) / 2) / u128::from(ppq);
    i64::try_from(scaled).map_or(MAX_TICK, |t| t.min(MAX_TICK))
}

/// One channel of one MIDI track while it is read.
#[derive(Default)]
struct Channel {
    /// Sounding notes by key: start tick and velocity.
    open: BTreeMap<u8, (i64, f32)>,
    notes: Vec<Note>,
    lanes: BTreeMap<ControllerKind, Vec<AutomationPoint>>,
    programs: Vec<ProgramChange>,
    /// The bank select bytes (`CC 0`, `CC 32`) last seen, with their tick: a program change at
    /// that tick takes them.
    bank: Option<(i64, Option<u8>, Option<u8>)>,
    /// The expression of the notes of an MPE zone's member channels, by note start and key.
    expr: BTreeMap<(i64, u8), Curves>,
}

/// The expression points of one imported note: `(ticks after the note start, value)` in file
/// order.
#[derive(Default, Clone)]
struct Curves {
    pitch: Vec<(i64, f32)>,
    pressure: Vec<(i64, f32)>,
    timbre: Vec<(i64, f32)>,
}

/// What an MPE member channel holds while a file is read.
#[derive(Clone, Copy)]
struct Member {
    /// The note sounding on the channel: key and start tick.
    note: Option<(u8, i64)>,
    bend: f32,
    pressure: f32,
    slide: Option<f32>,
    /// The registered parameter the select controllers name, and the bend range entered for it.
    rpn: [u8; 2],
    semitones: u8,
    cents: u8,
}

impl Member {
    const fn new() -> Self {
        Self {
            note: None,
            bend: 0.0,
            pressure: 0.0,
            slide: None,
            rpn: [127, 127],
            semitones: MEMBER_BEND_RANGE as u8,
            cents: 0,
        }
    }

    fn range(&self) -> f32 {
        f32::from(self.semitones) + f32::from(self.cents) * 0.01
    }
}

/// The MPE zones a track announces: its MPE Configuration Messages (RPN 6 on a master channel),
/// applied in file order.
fn announced_zones(events: &[SmfEvent]) -> MpeZones {
    let mut zones = MpeZones::default();
    let mut rpn = [[127u8; 2]; 16];
    for e in events {
        let SmfMessage::Controller {
            channel,
            controller,
            value,
        } = &e.message
        else {
            continue;
        };
        let r = &mut rpn[usize::from(*channel & 0x0F)];
        match *controller {
            101 => r[0] = *value,
            100 => r[1] = *value,
            98 | 99 => *r = [127, 127],
            6 if *r == [0, 6] => {
                zones.config(*channel, *value);
            }
            _ => {}
        }
    }
    zones
}

impl Channel {
    /// A note-off: the note lasts at least one tick.
    fn close(&mut self, key: u8, end: i64) {
        if let Some((start, velocity)) = self.open.remove(&key) {
            self.notes
                .push(Note::new(start, (end - start).max(1), key, velocity));
        }
    }

    /// A note-on: a sounding note of the key ends here, unless it started here too (it is
    /// replaced).
    fn open_note(&mut self, key: u8, tick: i64, velocity: f32) {
        if let Some((start, v)) = self.open.remove(&key)
            && tick > start
        {
            self.notes.push(Note::new(start, tick - start, key, v));
        }
        self.open.insert(key, (tick, velocity));
    }

    /// A bank select byte (`CC 0` or `CC 32`) at `tick`.
    fn bank_byte(&mut self, controller: u8, tick: i64, value: u8) {
        let (msb, lsb) = self.bank_at(tick);
        self.bank = Some(match controller {
            0 => (tick, Some(value), lsb),
            _ => (tick, msb, Some(value)),
        });
    }

    /// The bank select bytes seen at `tick`.
    fn bank_at(&self, tick: i64) -> (Option<u8>, Option<u8>) {
        match self.bank {
            Some((t, msb, lsb)) if t == tick => (msb, lsb),
            _ => (None, None),
        }
    }

    /// A program change: the bank select just before it at the same tick joins it, and its
    /// controller points leave the lanes.
    fn program(&mut self, tick: i64, program: u8) {
        let (msb, lsb) = self.bank_at(tick);
        for (controller, byte) in [(0u8, msb), (32, lsb)] {
            if byte.is_none() {
                continue;
            }
            let kind = ControllerKind::Cc(controller);
            if let Some(points) = self.lanes.get_mut(&kind) {
                points.retain(|p| p.tick != tick);
                if points.is_empty() {
                    self.lanes.remove(&kind);
                }
            }
        }
        self.programs.push(ProgramChange {
            tick,
            program,
            bank_msb: msb,
            bank_lsb: lsb,
        });
    }

    fn point(&mut self, kind: ControllerKind, tick: i64, value: f32) {
        let points = self.lanes.entry(kind).or_default();
        let point = AutomationPoint {
            tick,
            value,
            curve: Curve::Step,
        };
        match points.last_mut() {
            Some(last) if last.tick == tick => *last = point,
            _ => points.push(point),
        }
    }

    /// The controller lanes of a channel without notes, canonical.
    fn into_lanes(self) -> Vec<ControllerLane> {
        self.lanes
            .into_iter()
            .map(|(kind, points)| ControllerLane::new(kind, points))
            .collect()
    }

    /// Ends the hanging notes at `end` (at least a sixteenth long) and returns the finished
    /// track data, or `None` for a channel without notes.
    fn finish(mut self, end: i64) -> Option<ChannelData> {
        for (key, (start, velocity)) in std::mem::take(&mut self.open) {
            let length = if end > start { end - start } else { PPQ / 4 };
            self.notes.push(Note::new(start, length, key, velocity));
        }
        if self.notes.is_empty() {
            return None;
        }
        let mut notes = self.notes;
        notes.sort_by_key(|n| (n.tick, n.key));
        notes.dedup_by_key(|n| (n.tick, n.key));
        let lanes = self
            .lanes
            .into_iter()
            .map(|(kind, points)| ControllerLane::new(kind, points))
            .collect();
        let expression = self
            .expr
            .into_iter()
            .filter(|(at, _)| notes.binary_search_by_key(at, |n| (n.tick, n.key)).is_ok())
            .map(|((tick, key), c)| {
                let mut e = NoteExpression::new(tick, key);
                let curve = |raw: Vec<(i64, f32)>, kind: ExpressionKind| {
                    raw.into_iter()
                        .map(|(t, v)| ExpressionPoint::new(t, kind.clamp(v)))
                        .take(MAX_EXPRESSION_POINTS)
                        .collect()
                };
                e.pitch = curve(c.pitch, ExpressionKind::Pitch);
                e.pressure = curve(c.pressure, ExpressionKind::Pressure);
                e.timbre = curve(c.timbre, ExpressionKind::Timbre);
                e
            })
            .collect();
        let programs = MidiData {
            programs: self.programs,
            sysex: Vec::new(),
            expression,
        };
        Some((notes, lanes, programs))
    }
}

/// What a channel with notes yields: the notes, the controller lanes and the program changes
/// (in the `programs` of a [`MidiData`]).
type ChannelData = (Vec<Note>, Vec<ControllerLane>, MidiData);

/// `data` in canonical form and within the limits a clip keeps (the first ones win).
fn limited(data: MidiData) -> MidiData {
    let mut data = data.canonical();
    data.programs.truncate(MAX_PROGRAM_CHANGES);
    data.sysex.retain(|s| s.data.len() <= MAX_SYSEX_BYTES);
    data.sysex.truncate(MAX_SYSEX_EVENTS);
    data
}

fn level(v: u8) -> f32 {
    f32::from(v) / 127.0
}

/// Controller numbers that are not music data: data entry, (N)RPN selection, channel mode.
fn skipped_controller(n: u8) -> bool {
    matches!(n, 6 | 38 | 98..=101) || n >= 120
}

fn clean_name(text: &str) -> Option<String> {
    let name: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_MARKER_NAME + 16)
        .collect();
    let name = name.trim();
    let name: String = name.chars().take(MAX_MARKER_NAME).collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Reads a MIDI file as a song's worth of data (see the module docs).
pub fn read_song(bytes: &[u8]) -> Result<MidiSong, MidiFileError> {
    let smf = read(bytes)?;
    let ppq = smf.ppq;
    let mut tracks = Vec::new();
    let mut skipped = 0;
    let mut tempi: Vec<(i64, u32)> = Vec::new();
    let mut signatures: Vec<(i64, u8, u8)> = Vec::new();
    let mut markers = Vec::new();
    let mut chord_marks: Vec<(i64, ChordSymbol)> = Vec::new();
    let mut key_sig: Option<(i64, Key)> = None;
    let mut file_end = 0i64;
    for (index, t) in smf.tracks.iter().enumerate() {
        let end = model_tick(t.end_tick, ppq);
        file_end = file_end.max(end);
        let mut channels: BTreeMap<u8, Channel> = BTreeMap::new();
        let mut sysex: Vec<SysexEvent> = Vec::new();
        let mut title = None;
        // An MPE track: the zones it announces, and what each member channel holds.
        let zones = announced_zones(&t.events);
        let mut members = [Member::new(); 16];
        for e in &t.events {
            let tick = model_tick(e.tick, ppq);
            if !zones.is_empty()
                && member_message(&zones, &e.message, tick, &mut channels, &mut members)
            {
                continue;
            }
            let eff = |channel: u8| match zones.role(channel) {
                ChannelRole::Member(side) => side.master(),
                _ => channel,
            };
            match &e.message {
                SmfMessage::NoteOn {
                    channel,
                    key,
                    velocity,
                } if *velocity > 0 => {
                    channels
                        .entry(*channel)
                        .or_default()
                        .open_note(*key, tick, level(*velocity))
                }
                SmfMessage::NoteOn { channel, key, .. }
                | SmfMessage::NoteOff { channel, key, .. } => {
                    channels.entry(eff(*channel)).or_default().close(*key, tick);
                }
                SmfMessage::PolyPressure {
                    channel,
                    key,
                    pressure,
                } => channels.entry(eff(*channel)).or_default().point(
                    ControllerKind::PolyPressure(*key),
                    tick,
                    level(*pressure),
                ),
                SmfMessage::Controller {
                    channel,
                    controller,
                    value,
                } if !skipped_controller(*controller) => {
                    let c = channels.entry(eff(*channel)).or_default();
                    if matches!(*controller, 0 | 32) {
                        c.bank_byte(*controller, tick, *value);
                    }
                    c.point(ControllerKind::Cc(*controller), tick, level(*value))
                }
                SmfMessage::ChannelPressure { channel, pressure } => channels
                    .entry(*channel)
                    .or_default()
                    .point(ControllerKind::ChannelPressure, tick, level(*pressure)),
                SmfMessage::PitchBend { channel, value } => {
                    channels.entry(eff(*channel)).or_default().point(
                        ControllerKind::PitchBend,
                        tick,
                        ((f32::from(*value) - 8192.0) / 8192.0).clamp(-1.0, 1.0),
                    )
                }
                SmfMessage::Tempo(micros) => tempi.push((tick, *micros)),
                SmfMessage::TimeSignature {
                    numerator,
                    denominator_power,
                } => signatures.push((tick, *numerator, *denominator_power)),
                SmfMessage::Marker(text) => {
                    // A marker that reads as a chord symbol is a chord, not a marker.
                    if let Ok(chord) = parse_chord(text) {
                        chord_marks.push((tick, chord));
                    } else if let Some(name) = clean_name(text) {
                        markers.push((tick, name));
                    }
                }
                SmfMessage::Text(text) | SmfMessage::Lyric(text) => {
                    if let Ok(chord) = parse_chord(text) {
                        chord_marks.push((tick, chord));
                    }
                }
                SmfMessage::KeySignature { sharps, minor } => {
                    if key_sig.is_none_or(|(t, _)| tick < t) {
                        key_sig = Some((tick, key_of_signature(*sharps, *minor)));
                    }
                }
                SmfMessage::TrackName(text) => {
                    if title.is_none() {
                        title = clean_name(text);
                    }
                }
                SmfMessage::Program { channel, program } => {
                    channels
                        .entry(*channel)
                        .or_default()
                        .program(tick, *program);
                }
                SmfMessage::SysEx(data) => sysex.push(SysexEvent {
                    tick,
                    data: data.clone(),
                }),
                SmfMessage::Controller { .. } => {}
            }
        }
        let mut finished: Vec<(u8, ChannelData)> = Vec::new();
        // A channel with controllers and no notes: its lanes are the track's own.
        let mut lone: Vec<(u8, Vec<ControllerLane>)> = Vec::new();
        for (ch, c) in channels {
            let noteless = c.notes.is_empty() && c.open.is_empty();
            let programs = !c.programs.is_empty();
            if noteless {
                let lanes = c.into_lanes();
                if !lanes.is_empty() {
                    lone.push((ch, lanes));
                } else if programs {
                    skipped += 1;
                }
            } else if let Some(done) = c.finish(end) {
                finished.push((ch, done));
            }
        }
        // A SysEx message belongs to the file track, not to a channel: the first clip of the
        // track carries it, or a clip without notes when the track has none.
        let base = title.unwrap_or_else(|| format!("Track {}", index + 1));
        let own = |tracks: &[(u8, Vec<ControllerLane>)]| {
            join_lanes(tracks.iter().flat_map(|(_, lanes)| lanes))
        };
        if finished.is_empty() {
            if lone.is_empty() {
                if !sysex.is_empty() {
                    tracks.push(MidiTrack {
                        name: base,
                        notes: Vec::new(),
                        controllers: Vec::new(),
                        midi: limited(MidiData {
                            programs: Vec::new(),
                            sysex,
                            ..Default::default()
                        }),
                        track_controllers: Vec::new(),
                        end,
                    });
                }
                continue;
            }
            let mut sysex = Some(sysex);
            let several = lone.len() > 1;
            for (channel, lanes) in lone {
                tracks.push(MidiTrack {
                    name: if several {
                        format!("{base} ch{}", channel + 1)
                    } else {
                        base.clone()
                    },
                    notes: Vec::new(),
                    controllers: Vec::new(),
                    midi: limited(MidiData {
                        programs: Vec::new(),
                        sysex: sysex.take().unwrap_or_default(),
                        ..Default::default()
                    }),
                    track_controllers: lanes,
                    end,
                });
            }
            continue;
        }
        let mut sysex = Some(sysex);
        let mut lone = Some(own(&lone));
        let several = finished.len() > 1;
        for (channel, (notes, controllers, mut midi)) in finished {
            midi.sysex = sysex.take().unwrap_or_default();
            tracks.push(MidiTrack {
                name: if several {
                    format!("{base} ch{}", channel + 1)
                } else {
                    base.clone()
                },
                notes,
                controllers,
                midi: limited(midi),
                track_controllers: lone.take().unwrap_or_default(),
                end,
            });
        }
    }
    if tracks.is_empty() {
        return Err(MidiFileError::NoNotes);
    }
    markers.sort_by_key(|m: &(i64, String)| m.0);
    let (mut markers, mut ranges) = pair_ranges(markers);
    markers.truncate(MAX_IMPORTED_MARKERS);
    ranges.truncate(MAX_IMPORTED_MARKERS);
    let end = tracks.iter().map(|t| t.end).fold(file_end, i64::max);
    Ok(MidiSong {
        tracks,
        tempo: tempo_map(&tempi),
        meter: meter_map(&signatures),
        markers,
        ranges,
        skipped,
        chords: chords_of_marks(chord_marks, end),
        key: key_sig.map(|(_, k)| k),
    })
}

/// The key of a key signature: `sharps` above 0 (flats below) on the circle of fifths, the major
/// key or its relative minor.
fn key_of_signature(sharps: i8, minor: bool) -> Key {
    let major = (i32::from(sharps) * 7).rem_euclid(12) as u8;
    if minor {
        Key::new((major + 9) % 12, ScaleKind::NaturalMinor)
    } else {
        Key::new(major, ScaleKind::Major)
    }
}

/// Chord symbols at ticks as chords of the lane: each one lasts until the next (the last one
/// until `end`, at least a beat); of two at one tick the later wins.
fn chords_of_marks(mut marks: Vec<(i64, ChordSymbol)>, end: i64) -> Vec<ChordEvent> {
    marks.sort_by_key(|(t, _)| *t);
    let mut kept: Vec<(i64, ChordSymbol)> = Vec::new();
    for m in marks {
        match kept.last_mut() {
            Some(last) if last.0 == m.0 => *last = m,
            _ => kept.push(m),
        }
    }
    kept.truncate(atomik_model::MAX_CHORDS);
    let mut out = Vec::with_capacity(kept.len());
    for (i, (tick, c)) in kept.iter().enumerate() {
        let next = kept
            .get(i + 1)
            .map_or_else(|| end.max(tick + PPQ), |(t, _)| *t);
        let next = next.min(atomik_model::song::MAX_TICK);
        if next <= *tick {
            continue;
        }
        out.push(ChordEvent {
            bass: c.bass,
            ..ChordEvent::new(*tick, next - tick, c.root, c.quality)
        });
    }
    out
}

/// The chords of the notes of `tracks`, one guess per beat with the chord templates of the audio
/// chord detection (`atomik_dsp::harmony::recognise_chords`): each pitch class weighs the time its
/// notes sound in the beat times their velocity, and the lowest note of the beat is its bass.
/// Equal neighbours are one chord; a beat with no note is a gap.
pub fn detect_chords_from_notes(tracks: &[MidiTrack]) -> Vec<ChordEvent> {
    use atomik_dsp::harmony::{BeatChroma, ChordOptions, recognise_chords};
    let notes: Vec<&Note> = tracks.iter().flat_map(|t| t.notes.iter()).collect();
    let end = notes.iter().map(|n| n.tick + n.length).max().unwrap_or(0);
    if end <= 0 {
        return Vec::new();
    }
    let beats = ((end + PPQ - 1) / PPQ).min(atomik_model::MAX_CHORDS as i64 * 4) as usize;
    let mut units = vec![BeatChroma::default(); beats];
    let mut lowest: Vec<Option<(u8, f32)>> = vec![None; beats];
    for n in &notes {
        let (a, b) = (n.tick.max(0), (n.tick + n.length).min(beats as i64 * PPQ));
        if b <= a {
            continue;
        }
        for beat in (a / PPQ)..=((b - 1) / PPQ) {
            let (u0, u1) = (beat * PPQ, (beat + 1) * PPQ);
            let overlap = (b.min(u1) - a.max(u0)) as f32 / PPQ as f32;
            let w = overlap * n.velocity.max(0.05);
            let i = beat as usize;
            let u = &mut units[i];
            u.chroma[usize::from(n.key % 12)] += w;
            u.level += w;
            u.frames = 1;
            u.voiced = 1;
            if lowest[i].is_none_or(|(k, _)| n.key < k) {
                lowest[i] = Some((n.key, w));
            }
        }
    }
    for (u, low) in units.iter_mut().zip(&lowest) {
        if let Some((k, w)) = low {
            u.bass[usize::from(k % 12)] += *w;
        }
    }
    recognise_chords(&units, &ChordOptions::default())
        .into_iter()
        .map(|c| ChordEvent {
            bass: c.bass,
            ..ChordEvent::new(
                c.first as i64 * PPQ,
                (c.end - c.first) as i64 * PPQ,
                c.root,
                c.quality,
            )
        })
        .collect()
}

/// Handles the messages of a track that belong to an MPE member channel's own state: its notes
/// (they join the notes of the zone's master channel), the bend, pressure and CC 74 that are those
/// notes' expression, and the registered parameters of the channel. `true` when `message` was one
/// of them (anything else is read as on the master channel).
fn member_message(
    zones: &MpeZones,
    message: &SmfMessage,
    tick: i64,
    channels: &mut BTreeMap<u8, Channel>,
    members: &mut [Member; 16],
) -> bool {
    let channel = match message {
        SmfMessage::NoteOn { channel, .. }
        | SmfMessage::NoteOff { channel, .. }
        | SmfMessage::PitchBend { channel, .. }
        | SmfMessage::ChannelPressure { channel, .. }
        | SmfMessage::Controller { channel, .. } => *channel,
        _ => return false,
    };
    let ChannelRole::Member(side) = zones.role(channel) else {
        return false;
    };
    let master = channels.entry(side.master()).or_default();
    let m = &mut members[usize::from(channel & 0x0F)];
    // The curves of the note sounding on the channel (the point is `tick` after its start).
    let push = |m: &Member,
                master: &mut Channel,
                pick: fn(&mut Curves) -> &mut Vec<(i64, f32)>,
                v: f32| {
        if let Some((key, start)) = m.note {
            pick(master.expr.entry((start, key)).or_default()).push((tick - start, v));
        }
    };
    match message {
        SmfMessage::NoteOn { key, velocity, .. } if *velocity > 0 => {
            master.open_note(*key, tick, level(*velocity));
            m.note = Some((*key, tick));
            let (bend, range) = (m.bend, m.range());
            if bend != 0.0 {
                push(m, master, |c| &mut c.pitch, bend * range);
            }
            if m.pressure != 0.0 {
                push(m, master, |c| &mut c.pressure, m.pressure);
            }
            if let Some(slide) = m.slide.filter(|s| *s != 0.0) {
                push(m, master, |c| &mut c.timbre, slide);
            }
        }
        SmfMessage::NoteOn { key, .. } | SmfMessage::NoteOff { key, .. } => {
            master.close(*key, tick);
            if m.note.is_some_and(|(k, _)| k == *key) {
                m.note = None;
            }
        }
        SmfMessage::PitchBend { value, .. } => {
            m.bend = ((f32::from(*value) - 8192.0) / 8192.0).clamp(-1.0, 1.0);
            let v = m.bend * m.range();
            push(m, master, |c| &mut c.pitch, v);
        }
        SmfMessage::ChannelPressure { pressure, .. } => {
            m.pressure = level(*pressure);
            let v = m.pressure;
            push(m, master, |c| &mut c.pressure, v);
        }
        SmfMessage::Controller {
            controller, value, ..
        } => match *controller {
            74 => {
                let v = level(*value);
                m.slide = Some(v);
                push(m, master, |c| &mut c.timbre, v);
            }
            101 => m.rpn[0] = *value,
            100 => m.rpn[1] = *value,
            98 | 99 => m.rpn = [127, 127],
            6 if m.rpn == [0, 0] => m.semitones = (*value).min(96),
            38 if m.rpn == [0, 0] => m.cents = (*value).min(99),
            // Data entry for a registered parameter other than the bend range, and the other
            // controllers: the zone's master channel takes them.
            6 | 38 => {}
            _ => return false,
        },
        _ => return false,
    }
    true
}

/// The suffix the export gives the marker that ends a range.
const END_SUFFIX: &str = " (end)";

/// Splits `markers` (sorted by tick) into the plain markers and the ranges: a marker named
/// `<name> (end)` closes the latest earlier marker named `<name>` that no range has taken.
fn pair_ranges(markers: Vec<(i64, String)>) -> (Vec<(i64, String)>, Vec<ImportedRange>) {
    let mut taken = vec![false; markers.len()];
    let mut ranges = Vec::new();
    for (i, (end, name)) in markers.iter().enumerate() {
        let Some(base) = name
            .strip_suffix(END_SUFFIX)
            .filter(|b| !b.trim().is_empty())
        else {
            continue;
        };
        let start = (0..i)
            .rev()
            .find(|&j| !taken[j] && markers[j].1 == base && markers[j].0 < *end);
        if let Some(j) = start {
            taken[i] = true;
            taken[j] = true;
            ranges.push((markers[j].0, *end, base.to_owned()));
        }
    }
    ranges.sort_by_key(|r| r.0);
    let rest = markers
        .into_iter()
        .zip(taken)
        .filter_map(|(m, used)| (!used).then_some(m))
        .collect();
    (rest, ranges)
}

fn tempo_map(events: &[(i64, u32)]) -> Option<Vec<TempoEvent>> {
    if events.is_empty() {
        return None;
    }
    let mut sorted = events.to_vec();
    sorted.sort_by_key(|e| e.0);
    let mut map: Vec<TempoEvent> = Vec::new();
    for (tick, micros) in sorted {
        let event = TempoEvent::step(tick, bpm_of(micros).clamp(20.0, 999.0));
        match map.last_mut() {
            Some(last) if last.tick == tick => *last = event,
            _ => map.push(event),
        }
    }
    if map[0].tick != 0 {
        // Before the first event the file's tempo is the MIDI default.
        map.insert(0, TempoEvent::step(0, 120.0));
    }
    Some(map)
}

/// The meter map of the signatures `(tick, numerator, denominator power)`: a signature in the
/// middle of a bar takes effect at the next bar line, unsupported ones are skipped.
fn meter_map(events: &[(i64, u8, u8)]) -> Option<Vec<MeterEvent>> {
    if events.is_empty() {
        return None;
    }
    let mut sorted = events.to_vec();
    sorted.sort_by_key(|e| e.0);
    let mut map = vec![MeterEvent::new(1, 4, 4)];
    // The bar the current signature started at, and that bar's tick.
    let (mut bar, mut bar_tick) = (1u32, 0i64);
    let mut current = (4u8, 4u8);
    for (tick, num, power) in sorted {
        let den = 1u16.checked_shl(u32::from(power)).unwrap_or(0);
        let den = u8::try_from(den).unwrap_or(0);
        if !atomik_core::meter_ok(num, den) {
            continue;
        }
        let bar_ticks = bar_ticks(current);
        let whole = ((tick - bar_tick).max(0) + bar_ticks - 1) / bar_ticks;
        let at = u32::try_from(whole).map_or(u32::MAX, |w| bar.saturating_add(w));
        if (num, den) == current {
            continue;
        }
        bar_tick += whole * bar_ticks;
        bar = at;
        current = (num, den);
        match map.last_mut() {
            Some(last) if last.bar == at => *last = MeterEvent::new(at, num, den),
            _ => map.push(MeterEvent::new(at, num, den)),
        }
    }
    Some(map)
}

/// Lanes of the same kind joined into one (the later point wins at a shared tick), canonical.
fn join_lanes<'a>(lanes: impl Iterator<Item = &'a ControllerLane>) -> Vec<ControllerLane> {
    let mut by_kind: BTreeMap<ControllerKind, Vec<AutomationPoint>> = BTreeMap::new();
    for lane in lanes {
        by_kind
            .entry(lane.kind)
            .or_default()
            .extend(lane.points.iter().copied());
    }
    by_kind
        .into_iter()
        .map(|(kind, mut points)| {
            // A stable sort keeps the later lane's point last among equal ticks, and the last
            // of equal ticks wins.
            points.sort_by_key(|p| p.tick);
            points.reverse();
            points.dedup_by_key(|p| p.tick);
            points.reverse();
            ControllerLane::new(kind, points)
        })
        .collect()
}

impl MidiSong {
    /// Every track in one: the notes of all tracks and channels together (the first note wins at
    /// a tick and key shared by two) and the controller points of the same kind joined (the
    /// later track wins at a shared tick). What File > Import MIDI puts in one clip.
    pub fn merged(&self) -> MidiTrack {
        let mut notes: Vec<Note> = self.tracks.iter().flat_map(|t| t.notes.clone()).collect();
        notes.sort_by_key(|n| (n.tick, n.key));
        notes.dedup_by_key(|n| (n.tick, n.key));
        let controllers = join_lanes(
            self.tracks
                .iter()
                .flat_map(|t| t.controllers.iter().chain(&t.track_controllers)),
        );
        let mut midi = MidiData::default();
        for t in &self.tracks {
            midi.append(t.midi.clone());
        }
        MidiTrack {
            name: self
                .tracks
                .first()
                .map_or_else(String::new, |t| t.name.clone()),
            notes,
            controllers,
            midi: limited(midi),
            track_controllers: Vec::new(),
            end: self.tracks.iter().map(|t| t.end).max().unwrap_or(0),
        }
    }
}

impl MidiTrack {
    /// Whether the track has a clip: notes, controllers of its channel, or program changes and
    /// SysEx. A track with only [`Self::track_controllers`] has none.
    pub fn has_clip(&self) -> bool {
        !self.notes.is_empty() || !self.controllers.is_empty() || !self.midi.is_empty()
    }

    /// The track's own controller lanes at song tick `start` (the lanes of [`Self::track_controllers`]
    /// moved by `start`; a point is kept inside [`MAX_TICK`]).
    pub fn own_lanes(&self, start: i64) -> Vec<ControllerLane> {
        let moved = self.track_controllers.iter().map(|lane| {
            let points = lane
                .points
                .iter()
                .map(|p| AutomationPoint {
                    tick: (p.tick + start).min(MAX_TICK),
                    ..*p
                })
                .collect();
            ControllerLane::new(lane.kind, points)
        });
        // Points squeezed together at MAX_TICK by the clamp are joined (the last wins).
        join_lanes(moved.collect::<Vec<_>>().iter())
    }

    /// The content tick just past the last note, controller point, program change and SysEx
    /// message.
    fn content_end(&self) -> i64 {
        let notes = self.notes.iter().map(|n| n.tick + n.length);
        let points = self
            .controllers
            .iter()
            .flat_map(|l| l.points.iter().map(|p| p.tick + 1));
        let messages = self
            .midi
            .sysex
            .iter()
            .map(|s| s.tick + 1)
            .chain(self.midi.programs.iter().map(|p| p.tick + 1));
        notes.chain(points).chain(messages).max().unwrap_or(0)
    }

    /// A note clip at song tick `start`. Its length is the track's end when that covers the
    /// content, else the content's end rounded up to the next bar line of `song`; at least one
    /// tick and at most [`MAX_TICK`].
    pub fn into_clip(self, start: i64, song: &Song) -> Clip {
        let content = self.content_end();
        let length = if self.end >= content {
            self.end
        } else {
            let table = song.meter_table();
            let abs = table.map().next_bar_line((start + content - 1).max(0));
            (abs - start).max(content)
        };
        let length = length.clamp(1, MAX_TICK - start.min(MAX_TICK - 1));
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
            param_lanes: Vec::new(),
            controllers: self.controllers,
            midi: self.midi.into(),
            groove: None,
            content: ClipContent::Notes(self.notes.into()),
        }
    }
}

#[cfg(test)]
#[path = "midi_file_tests.rs"]
mod tests;
