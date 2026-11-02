//! MIDI files and the song: the conversion between a Standard MIDI File
//! ([`atomik_midi_io::smf`]) and the model's notes, controller lanes, tempo map, meter map and
//! markers. No file access and no document here; the app core reads and writes the files and
//! applies the result as commands.
//!
//! **Export** writes what plays, not the raw clips: a clip, a track or the song is flattened
//! with the model's own rules (`flatten_notes_with`, `flatten_controllers`: loops, offsets, swing, a
//! later clip hiding an earlier one, muted notes and clips left out), so the file sounds like
//! the arrangement.
//!
//! - The resolution is [`PPQ`](atomik_core::PPQ) (15 360 ticks per quarter note, a valid 15-bit
//!   division), so every model tick survives a round trip unchanged. Velocities are 7 bit
//!   (`round(v * 127)`, at least 1), pitch bend 14 bit, other controllers 7 bit.
//! - A clip or a track is a type 0 file (one `MTrk`, channel 1); the song is a type 1 file: the
//!   first `MTrk` holds the song name, tempo map, time signatures and markers, then one `MTrk`
//!   per note track (named after its device, channel `index mod 16`). Audio and automation
//!   lanes are not exported.
//! - Tempo ramps become a staircase (a tempo event every sixteenth note, at most 2048 per
//!   ramp); the time signature events sit on the bar lines; range markers are not exported,
//!   point markers are marker meta events. A clip file carries the tempo and time signature in
//!   effect where the clip starts, a track file the whole tempo map, meter map and markers.
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
//! (N)RPN numbers are skipped, and so are program changes and SysEx. Tempo, time signature and
//! marker events from every track make the song-level data. Ticks are rescaled from the file's
//! division with rounding.

use atomik_core::PPQ;
use atomik_midi_io::smf::{SmfError, SmfEvent, SmfFile, SmfMessage, SmfTrack, read};
use atomik_model::song::{
    AutomationPoint, Clip, ClipContent, ClipId, Curve, Lane, MAX_MARKER_NAME, MAX_TICK, MeterEvent,
    Note, Song, TempoEvent, Track, bar_ticks, flatten_controllers, flatten_notes_with,
};
use atomik_model::{ControllerKind, ControllerLane, DeviceKey};
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
    /// The file holds no notes (only tempo, markers or controllers).
    #[error("the MIDI file has no notes")]
    NoNotes,
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

/// Tempo map, time signatures (on the bar lines) and point markers of the song.
fn conductor_events(song: &Song) -> Vec<Timed> {
    let mut out = tempo_events(&song.tempo);
    let table = song.meter_table();
    for s in table.segments() {
        out.push(timed(s.tick, 1, time_signature(s.num, s.den)));
    }
    for m in &song.markers {
        out.push(timed(m.tick, 1, SmfMessage::Marker(m.name.clone())));
    }
    out
}

/// Note-on and note-off events of flattened notes `(tick, length, key, velocity)` on `channel`.
fn note_events(notes: &[(i64, i64, u8, f32)], channel: u8) -> Vec<Timed> {
    // Overlapping notes of one key are cut at the next one's start.
    let mut ends: Vec<i64> = notes.iter().map(|n| n.0 + n.1).collect();
    let mut next_of_key: BTreeMap<u8, usize> = BTreeMap::new();
    for i in (0..notes.len()).rev() {
        if let Some(&j) = next_of_key.get(&notes[i].2) {
            ends[i] = ends[i].min(notes[j].0);
        }
        next_of_key.insert(notes[i].2, i);
    }
    let mut out = Vec::new();
    for (n, end) in notes.iter().zip(ends) {
        let end = end.max(n.0 + 1);
        out.push(timed(
            n.0,
            4,
            SmfMessage::NoteOn {
                channel,
                key: n.2,
                velocity: velocity7(n.3),
            },
        ));
        out.push(timed(
            end,
            2,
            SmfMessage::NoteOff {
                channel,
                key: n.2,
                velocity: 0,
            },
        ));
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

/// Sorts the messages and turns them into a track ending at `end` (at least the last tick).
fn smf_track(mut events: Vec<Timed>, end: i64) -> SmfTrack {
    events.sort_by_key(|e| (e.tick, e.rank));
    let tick = |t: i64| t.max(0) as u64;
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

/// The notes and controllers a track plays, as events on `channel` (see the module docs).
fn play_events(track: &Track, channel: u8, tails: bool) -> Vec<Timed> {
    let mut out = note_events(&flatten_notes_with(track, tails), channel);
    out.extend(controller_events(&flatten_controllers(track), channel));
    out
}

fn file(format: u16, tracks: Vec<SmfTrack>) -> SmfFile {
    SmfFile {
        format,
        ppq: u16::try_from(PPQ).unwrap_or(u16::MAX),
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
pub fn clip_file(song: &Song, clip: &Clip, name: &str) -> Vec<u8> {
    let table = song.tempo_table();
    let meter = song.meter_table();
    let bpm = table.map().bpm(clip.start as f64);
    let (num, den) = meter.map().meter_at(clip.start);
    let mut events = vec![
        named(name),
        timed(0, 1, SmfMessage::Tempo(micros_of(bpm))),
        timed(0, 1, time_signature(num, den)),
    ];
    events.extend(play_events(&lone_clip_track(clip), 0, false));
    file(0, vec![smf_track(events, clip.length)]).to_bytes()
}

/// A track as a type 0 file: its notes and controllers at their song ticks, with the song's
/// tempo map, time signatures and markers.
pub fn track_file(song: &Song, track: &Track, name: &str) -> Vec<u8> {
    let mut events = vec![named(name)];
    events.extend(conductor_events(song));
    events.extend(play_events(track, 0, song.tails));
    file(0, vec![smf_track(events, 0)]).to_bytes()
}

/// The song as a type 1 file: a conductor track (name, tempo map, time signatures, markers) and
/// one track per note track, named by `name_of` (see the module docs).
pub fn song_file(song: &Song, name: &str, name_of: &dyn Fn(&DeviceKey) -> String) -> Vec<u8> {
    let mut events = vec![named(name)];
    events.extend(conductor_events(song));
    let mut tracks = vec![smf_track(events, 0)];
    let note_tracks = song
        .tracks
        .iter()
        .filter(|t| t.lanes.first().is_some_and(|l| l.kind.is_notes()));
    for (i, track) in note_tracks.enumerate() {
        let channel = (i % 16) as u8;
        let mut events = vec![named(&name_of(&track.device))];
        events.extend(play_events(track, channel, song.tails));
        tracks.push(smf_track(events, 0));
    }
    file(1, tracks).to_bytes()
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
    /// The markers as `(tick, name)`.
    pub markers: Vec<(i64, String)>,
    /// How many channels with controllers but no notes were left out.
    pub skipped: usize,
}

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

    /// Ends the hanging notes at `end` (at least a sixteenth long) and returns the finished
    /// track data, or `None` for a channel without notes.
    fn finish(mut self, end: i64) -> Option<(Vec<Note>, Vec<ControllerLane>)> {
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
        Some((notes, lanes))
    }
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
    for (index, t) in smf.tracks.iter().enumerate() {
        let end = model_tick(t.end_tick, ppq);
        let mut channels: BTreeMap<u8, Channel> = BTreeMap::new();
        let mut title = None;
        for e in &t.events {
            let tick = model_tick(e.tick, ppq);
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
                    channels.entry(*channel).or_default().close(*key, tick);
                }
                SmfMessage::PolyPressure {
                    channel,
                    key,
                    pressure,
                } => channels.entry(*channel).or_default().point(
                    ControllerKind::PolyPressure(*key),
                    tick,
                    level(*pressure),
                ),
                SmfMessage::Controller {
                    channel,
                    controller,
                    value,
                } if !skipped_controller(*controller) => channels
                    .entry(*channel)
                    .or_default()
                    .point(ControllerKind::Cc(*controller), tick, level(*value)),
                SmfMessage::ChannelPressure { channel, pressure } => channels
                    .entry(*channel)
                    .or_default()
                    .point(ControllerKind::ChannelPressure, tick, level(*pressure)),
                SmfMessage::PitchBend { channel, value } => {
                    channels.entry(*channel).or_default().point(
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
                    if let Some(name) = clean_name(text) {
                        markers.push((tick, name));
                    }
                }
                SmfMessage::TrackName(text) => {
                    if title.is_none() {
                        title = clean_name(text);
                    }
                }
                SmfMessage::Controller { .. } | SmfMessage::Program { .. } => {}
            }
        }
        let finished: Vec<(u8, Vec<Note>, Vec<ControllerLane>)> = channels
            .into_iter()
            .filter_map(|(ch, c)| {
                let has_lanes = !c.lanes.is_empty();
                let done = c.finish(end);
                if done.is_none() && has_lanes {
                    skipped += 1;
                }
                done.map(|(n, l)| (ch, n, l))
            })
            .collect();
        let several = finished.len() > 1;
        let base = title.unwrap_or_else(|| format!("Track {}", index + 1));
        for (channel, notes, controllers) in finished {
            tracks.push(MidiTrack {
                name: if several {
                    format!("{base} ch{}", channel + 1)
                } else {
                    base.clone()
                },
                notes,
                controllers,
                end,
            });
        }
    }
    if tracks.is_empty() {
        return Err(MidiFileError::NoNotes);
    }
    markers.sort_by_key(|m: &(i64, String)| m.0);
    markers.truncate(MAX_IMPORTED_MARKERS);
    Ok(MidiSong {
        tracks,
        tempo: tempo_map(&tempi),
        meter: meter_map(&signatures),
        markers,
        skipped,
    })
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

impl MidiSong {
    /// Every track in one: the notes of all tracks and channels together (the first note wins at
    /// a tick and key shared by two) and the controller points of the same kind joined (the
    /// later track wins at a shared tick). What File > Import MIDI puts in one clip.
    pub fn merged(&self) -> MidiTrack {
        let mut notes: Vec<Note> = self.tracks.iter().flat_map(|t| t.notes.clone()).collect();
        notes.sort_by_key(|n| (n.tick, n.key));
        notes.dedup_by_key(|n| (n.tick, n.key));
        let mut lanes: BTreeMap<ControllerKind, Vec<AutomationPoint>> = BTreeMap::new();
        for lane in self.tracks.iter().flat_map(|t| &t.controllers) {
            lanes
                .entry(lane.kind)
                .or_default()
                .extend(lane.points.iter().copied());
        }
        let controllers = lanes
            .into_iter()
            .map(|(kind, mut points)| {
                points.sort_by_key(|p| p.tick);
                // The last of equal ticks wins.
                points.reverse();
                points.dedup_by_key(|p| p.tick);
                points.reverse();
                ControllerLane::new(kind, points)
            })
            .collect();
        MidiTrack {
            name: self
                .tracks
                .first()
                .map_or_else(String::new, |t| t.name.clone()),
            notes,
            controllers,
            end: self.tracks.iter().map(|t| t.end).max().unwrap_or(0),
        }
    }
}

impl MidiTrack {
    /// The content tick just past the last note and controller point.
    fn content_end(&self) -> i64 {
        let notes = self.notes.iter().map(|n| n.tick + n.length);
        let points = self
            .controllers
            .iter()
            .flat_map(|l| l.points.iter().map(|p| p.tick + 1));
        notes.chain(points).max().unwrap_or(0)
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
            controllers: self.controllers,
            content: ClipContent::Notes(self.notes.into()),
        }
    }
}

#[cfg(test)]
#[path = "midi_file_tests.rs"]
mod tests;
