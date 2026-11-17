//! Tab files and the song: a [`TabScore`] (what the Guitar Pro, MusicXML and text readers make)
//! turned into song tracks and clips, and song tracks and clips turned back into a score.
//!
//! **Import** ([`tab_song`]) gives one track per tab track. Its clip holds the notes (the
//! sounding pitch of each string and fret, ties joined into one long note), the tab sidecar
//! (instrument, tuning, capo, the string, fret and techniques of every note, chord diagrams and
//! lyrics) and the expression the techniques render to (`tab_render::render_all`: bends, slides,
//! vibrato, palm mute, dead and harmonic notes). Percussion tracks hold their drum keys as plain
//! notes without a sidecar. The tempo map, the meter map, the section markers, the key and the
//! chord symbols travel as they do for a MIDI import (`MidiSong`), so the same code puts them in
//! the song. Every track plays the instrument [`instrument_for`] names.
//!
//! **Export** ([`score_from_song`]) goes the other way for a clip, a track or the whole song: the
//! notes of the played note lanes (loops and overlaps resolved, as the engine plays them), each
//! with the string and fret the clip's sidecar holds. A note without an entry is placed by the
//! fretter on the instrument the sidecar names, or, for a track that has none, on the instrument
//! the track's name suggests ([`instrument_for_name`], guitar when it says nothing). Notes that
//! no string can play are left out and counted in the warnings.

use crate::midi_file::{MidiSong, MidiTrack, chords_of_marks, key_of_signature};
use atomik_core::PPQ;
use atomik_model::song::{MAX_MARKER_NAME, MAX_TICK, Song, TempoEvent, audible_pieces};
use atomik_model::tab_data::MAX_TAB_NOTES;
use atomik_model::tab_edit::{self, TabEdit};
use atomik_model::{
    ChordSymbol, ClipContent, ClipId, DeviceKey, MeterEvent, MidiData, Note, TabData, TabDiagram,
    TabLyric, TabNoteRef, parse_chord,
};
use atomik_tab::fretting::FretOptions;
use atomik_tab::score::{DiagramRef, KeySignature, TabNote, TabScore, TabTrack};
use atomik_tab::{ImportWarning, gp, musicxml};
use std::collections::BTreeMap;

/// The biggest tab file read (the Guitar Pro limit; real files are a few hundred kilobytes).
pub const MAX_TAB_BYTES: usize = gp::limits::MAX_FILE_BYTES;

/// The instrument a track made by a tab import plays, for the tab instrument `instrument_id`
/// (`guitar6`, `bass4`, `banjo5`, ...; empty for a custom tuning).
///
/// One place decides it. Today every tab track plays what a MIDI import plays.
// phase D: Plucked Strings. Map the instrument id to the `atomik.plucked` device here (and make
// `MidiImport` take an instrument per track so a bass and a guitar get different patches).
pub fn instrument_for(_instrument_id: &str) -> &'static str {
    crate::app_core::IMPORT_INSTRUMENT
}

/// The catalogue instrument a track's name suggests (`guitar6` when it suggests none).
pub fn instrument_for_name(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["bass"]) {
        "bass4"
    } else if has(&["banjo"]) {
        "banjo5"
    } else if has(&["ukulele", "uke"]) {
        "ukulele_soprano"
    } else if has(&["mandolin"]) {
        "mandolin"
    } else if has(&["bouzouki"]) {
        "bouzouki_irish"
    } else if has(&["12-string", "12 string", "twelve"]) {
        "guitar12"
    } else {
        "guitar6"
    }
}

/// Whether a track name looks like a drum track (it is exported as a percussion part).
fn is_drum_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["drum", "kit", "perc", "snare", "kick", "hi-hat", "hihat"]
        .iter()
        .any(|w| n.contains(w))
}

// ------------------------------------------------------------------------------------ reading

/// The file formats the tab import reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabFormat {
    /// Guitar Pro (`.gp3`, `.gp4`, `.gp5`, `.gpx`, `.gp`).
    GuitarPro,
    /// MusicXML (`.musicxml`, `.xml`, `.mxl`).
    MusicXml,
}

/// Tells a tab file from its first bytes (the extension is not trusted), or `None`.
pub fn sniff_tab(bytes: &[u8]) -> Option<TabFormat> {
    // MusicXML first: a bare XML document is Guitar Pro's only when its root says so.
    if musicxml::sniff(bytes).is_some() {
        return Some(TabFormat::MusicXml);
    }
    gp::sniff(bytes).map(|_| TabFormat::GuitarPro)
}

/// Whether the file at `path` is a tab file: a Guitar Pro or MusicXML extension, or an `.xml`
/// whose first bytes are MusicXML (the file is opened for that).
pub fn is_tab_path(path: &std::path::Path) -> bool {
    use atomik_library::{FileKind, classify};
    if classify(path, false) == Some(FileKind::Tab) {
        return true;
    }
    let is_xml = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("xml"));
    if !is_xml {
        return false;
    }
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(4096).read_to_end(&mut head))
        .is_ok()
        && musicxml::sniff(&head).is_some()
}

/// Reads a tab file of any supported format: the score and what could not be carried over (each
/// warning as one line of text).
pub fn read_tab(bytes: &[u8]) -> Result<(TabScore, Vec<String>), String> {
    if bytes.len() > MAX_TAB_BYTES {
        return Err(format!(
            "the file is {} bytes, too large for a tab file",
            bytes.len()
        ));
    }
    let lines = |w: Vec<ImportWarning>| w.iter().map(ToString::to_string).collect::<Vec<_>>();
    match sniff_tab(bytes) {
        Some(TabFormat::GuitarPro) => gp::read_gp(bytes)
            .map(|(s, w)| (s, lines(w)))
            .map_err(|e| e.to_string()),
        Some(TabFormat::MusicXml) => musicxml::read_musicxml(bytes)
            .map(|(s, w)| (s, lines(w)))
            .map_err(|e| e.to_string()),
        None => Err("this is not a Guitar Pro or MusicXML file".to_owned()),
    }
}

// ------------------------------------------------------------------------------------ import

/// What a tab score becomes in the song.
#[derive(Debug, Clone)]
pub struct TabSong {
    /// The tracks, tempo map, meter map, markers, chords and key, as a MIDI import has them.
    pub song: MidiSong,
    /// The instrument of the first track (what [`instrument_for`] decides for the import).
    pub instrument: &'static str,
    /// What the conversion changed or left out.
    pub warnings: Vec<String>,
}

/// A note while a track is converted.
struct Placed {
    tick: i64,
    len: i64,
    key: u8,
    velocity: f32,
    string: u8,
    fret: u8,
    tech: atomik_tab::score::Techniques,
}

fn velocity_of(v: u8) -> f32 {
    (f32::from(v) / 127.0).clamp(1.0 / 127.0, 1.0)
}

fn clip_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// Converts `score` (see the module docs). Never fails: what cannot be converted is left out and
/// listed in [`TabSong::warnings`].
pub fn tab_song(score: &TabScore) -> TabSong {
    let mut warnings = Vec::new();
    let grid_end = score
        .bar_grid()
        .last()
        .map_or(0, |(start, len)| (start + len).min(MAX_TICK));
    let mut tracks = Vec::new();
    for (index, t) in score.tracks.iter().enumerate() {
        tracks.push(convert_track(t, index, grid_end, &mut warnings));
    }
    let mut marks: Vec<(i64, ChordSymbol)> = Vec::new();
    for t in &score.tracks {
        for (tick, c) in &t.chords {
            if let Ok(symbol) = parse_chord(&c.name()) {
                marks.push((*tick, symbol));
            }
        }
    }
    let chords = chords_of_marks(marks, grid_end);
    let key = score
        .key
        .first()
        .map(|(_, k)| key_of_signature(k.fifths, k.minor));
    let instrument = instrument_for(
        score
            .tracks
            .iter()
            .find(|t| !t.is_percussion)
            .map_or("", |t| t.instrument_id.as_str()),
    );
    TabSong {
        song: MidiSong {
            tracks,
            tempo: tempo_map(score),
            meter: meter_map(score),
            markers: markers(score),
            ranges: Vec::new(),
            skipped: 0,
            chords,
            key,
        },
        instrument,
        warnings,
    }
}

fn convert_track(
    t: &TabTrack,
    index: usize,
    grid_end: i64,
    warnings: &mut Vec<String>,
) -> MidiTrack {
    let name = if t.name.trim().is_empty() {
        format!("Track {}", index + 1)
    } else {
        t.name.trim().to_owned()
    };
    let mut placed: Vec<Placed> = Vec::new();
    let mut skipped = 0usize;
    if t.is_percussion {
        for n in &t.notes {
            if n.fret > 127 || n.tick > MAX_TICK {
                skipped += 1;
                continue;
            }
            placed.push(Placed {
                tick: n.tick,
                len: n.len,
                key: n.fret,
                velocity: velocity_of(n.velocity),
                string: n.string,
                fret: n.fret,
                tech: n.tech.clone(),
            });
        }
    } else {
        let mut last_on_string: BTreeMap<u8, usize> = BTreeMap::new();
        for n in &t.notes {
            let key = t.key_of(n).filter(|k| *k <= 127);
            let Some(key) = key.filter(|_| n.fret <= t.tuning.frets && n.tick <= MAX_TICK) else {
                skipped += 1;
                continue;
            };
            // A tied note lengthens the note it continues.
            if n.tie
                && let Some(&i) = last_on_string.get(&n.string)
                && placed[i].tick + placed[i].len >= n.tick - PPQ / 32
            {
                placed[i].len = (n.tick + n.len - placed[i].tick).max(placed[i].len);
                continue;
            }
            last_on_string.insert(n.string, placed.len());
            placed.push(Placed {
                tick: n.tick,
                len: n.len,
                key,
                velocity: velocity_of(n.velocity),
                string: n.string,
                fret: n.fret,
                tech: n.tech.clone(),
            });
        }
    }
    if skipped > 0 {
        warnings.push(format!(
            "{name}: {skipped} note{} outside the neck or the key range left out",
            if skipped == 1 { "" } else { "s" }
        ));
    }
    placed.sort_by_key(|p| (p.tick, p.key));
    placed.dedup_by_key(|p| (p.tick, p.key));
    let mut notes: Vec<Note> = placed
        .iter()
        .map(|p| Note::new(p.tick, p.len.max(1), p.key, p.velocity))
        .collect();
    let mut midi = MidiData::default();
    if !t.is_percussion {
        let id = if t.instrument_id.is_empty() {
            "custom"
        } else {
            t.instrument_id.as_str()
        };
        let mut tab = TabData::with_tuning(id, t.tuning.clone(), t.capo);
        tab.capo.fret = tab.capo.fret.min(atomik_tab::instrument::MAX_CAPO);
        tab.notes = placed
            .iter()
            .take(MAX_TAB_NOTES)
            .map(|p| TabNoteRef {
                tick: p.tick,
                key: p.key,
                string: p.string,
                fret: p.fret,
                tech: p.tech.clone(),
                plain: None,
            })
            .collect();
        let strings = t.tuning.len();
        tab.diagrams = t
            .diagrams
            .iter()
            .filter(|d: &&DiagramRef| d.voicing.frets.len() == strings)
            .map(|d| TabDiagram {
                tick: d.tick,
                name: clip_chars(&d.name, 64),
                voicing: d.voicing.clone(),
            })
            .collect();
        tab.lyrics = t
            .lyrics
            .iter()
            .map(|(tick, text)| TabLyric {
                tick: *tick,
                text: clip_chars(text, 64),
            })
            .collect();
        let tab = tab.canonical();
        match tab.check(MAX_TICK) {
            Ok(()) => midi.tab = Some(tab),
            Err(e) => warnings.push(format!("{name}: the tab view was left out ({e})")),
        }
        atomik_model::tab_render::render_all(&mut notes, &mut midi);
    }
    MidiTrack {
        name,
        notes,
        controllers: Vec::new(),
        midi,
        track_controllers: Vec::new(),
        end: grid_end,
    }
}

fn tempo_map(score: &TabScore) -> Option<Vec<TempoEvent>> {
    if score.tempo.is_empty() {
        return None;
    }
    let mut map: Vec<TempoEvent> = Vec::new();
    for (tick, bpm) in &score.tempo {
        let event = TempoEvent::step((*tick).clamp(0, MAX_TICK), bpm.clamp(20.0, 999.0));
        match map.last_mut() {
            Some(last) if last.tick == event.tick => *last = event,
            _ => map.push(event),
        }
    }
    if map[0].tick != 0 {
        map.insert(0, TempoEvent::step(0, 120.0));
    }
    Some(map)
}

fn meter_map(score: &TabScore) -> Option<Vec<MeterEvent>> {
    let mut map: Vec<MeterEvent> = Vec::new();
    for (bar0, num, den) in &score.meter {
        if !atomik_core::meter_ok(*num, *den) {
            continue;
        }
        let bar = bar0.saturating_add(1);
        match map.last_mut() {
            Some(last) if last.bar >= bar => *last = MeterEvent::new(last.bar, *num, *den),
            Some(last) if (last.num, last.den) == (*num, *den) => {}
            _ => map.push(MeterEvent::new(bar, *num, *den)),
        }
    }
    if map.is_empty() {
        return None;
    }
    if map[0].bar != 1 {
        map.insert(0, MeterEvent::new(1, 4, 4));
    }
    Some(map)
}

fn markers(score: &TabScore) -> Vec<(i64, String)> {
    score
        .markers
        .iter()
        .filter_map(|(tick, name)| {
            let name = clip_chars(name.trim(), MAX_MARKER_NAME);
            (!name.is_empty() && (0..=MAX_TICK).contains(tick)).then_some((*tick, name))
        })
        .take(crate::midi_file::MAX_IMPORTED_MARKERS)
        .collect()
}

// ------------------------------------------------------------------------------------ export

/// What an export writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabSource {
    /// One clip, from its own start.
    Clip(ClipId),
    /// One track.
    Track(DeviceKey),
    /// Every note track.
    Song,
}

/// The score of `what` in `song`: `title` names it and `name_of` gives the track names. Returns
/// the score and what could not be carried over; an error when there is nothing to write.
pub fn score_from_song(
    song: &Song,
    what: &TabSource,
    title: &str,
    name_of: &dyn Fn(&DeviceKey) -> String,
) -> Result<(TabScore, Vec<String>), String> {
    let mut warnings = Vec::new();
    let (devices, only, origin): (Vec<DeviceKey>, Option<ClipId>, i64) = match what {
        TabSource::Clip(id) => {
            let (track, _, clip) = song.clip(*id).ok_or("the clip is gone")?;
            (vec![track.device.clone()], Some(*id), clip.start)
        }
        TabSource::Track(device) => (vec![device.clone()], None, 0),
        TabSource::Song => (
            song.tracks
                .iter()
                .filter(|t| t.lanes.first().is_some_and(|l| l.kind.is_notes()))
                .map(|t| t.device.clone())
                .collect(),
            None,
            0,
        ),
    };
    let mut score = TabScore {
        title: title.to_owned(),
        ..TabScore::default()
    };
    for device in &devices {
        let Some(track) = song.track(device) else {
            continue;
        };
        let name = name_of(device);
        if let Some(t) = track_of(track, &name, only, origin, &mut warnings) {
            score.tracks.push(t);
        }
    }
    if score.tracks.is_empty() {
        return Err("there are no notes to write".to_owned());
    }
    // Song facts, as they are at the origin of the export.
    for e in &song.tempo {
        let t = e.tick - origin;
        if t >= 0 {
            score.tempo.push((t, e.bpm));
        }
    }
    if let Some(at) = song.tempo.iter().rfind(|e| e.tick <= origin)
        && score.tempo.first().is_none_or(|f| f.0 > 0)
    {
        score.tempo.insert(0, (0, at.bpm));
    }
    score.meter = if origin == 0 {
        song.meter
            .iter()
            .map(|m| (m.bar.saturating_sub(1), m.num, m.den))
            .collect()
    } else {
        let (n, d) = song.meter_table().map().meter_at(origin);
        vec![(0, n, d)]
    };
    if let Some(k) = song.harmony.key_at(origin) {
        score.key.push((0, key_signature_of(k)));
    }
    for c in song.harmony.changes.iter().filter(|c| c.tick > origin) {
        score.key.push((c.tick - origin, key_signature_of(c.key)));
    }
    score.markers = song
        .markers
        .iter()
        .filter(|m| m.tick >= origin)
        .map(|m| (m.tick - origin, m.name.clone()))
        .collect();
    let chords: Vec<(i64, atomik_tab::ChordSymbol)> = song
        .harmony
        .chords
        .iter()
        .filter(|c| c.tick >= origin)
        .filter_map(|c| {
            let name = atomik_model::format_chord(c.root, c.quality, c.bass);
            Some((c.tick - origin, atomik_tab::ChordSymbol::parse(&name).ok()?))
        })
        .collect();
    if let Some(first) = score.tracks.iter_mut().find(|t| !t.is_percussion) {
        first.chords = chords;
    }
    // The meter must have a first entry at bar 0 for the writers.
    if score.meter.first().is_none_or(|m| m.0 != 0) {
        score.meter.insert(0, (0, 4, 4));
    }
    for w in score.sanitize() {
        warnings.push(w);
    }
    Ok((score, warnings))
}

fn key_signature_of(k: atomik_model::Key) -> KeySignature {
    use atomik_model::theory::ScaleKind;
    let minor = matches!(
        k.scale,
        ScaleKind::NaturalMinor | ScaleKind::HarmonicMinor | ScaleKind::MelodicMinor
    );
    let major_root = if minor { (k.root + 3) % 12 } else { k.root };
    let fifths = ((i32::from(major_root) * 7 + 5) % 12 - 5) as i8;
    KeySignature { fifths, minor }
}

/// The tab track a song track makes, or `None` when it plays no notes.
fn track_of(
    track: &atomik_model::Track,
    name: &str,
    only: Option<ClipId>,
    origin: i64,
    warnings: &mut Vec<String>,
) -> Option<TabTrack> {
    let mut notes: Vec<Note> = Vec::new();
    let mut tab: Option<TabData> = None;
    for lane in track.played_note_lanes() {
        for (clip, s, e, cs) in audible_pieces(lane) {
            if only.is_some_and(|id| id != clip.id) {
                continue;
            }
            let ClipContent::Notes(list) = &clip.content else {
                continue;
            };
            let ce = cs + (e - s);
            let from = list.partition_point(|n| n.tick < cs);
            for n in list[from..].iter().take_while(|n| n.tick < ce) {
                let t = s + (n.tick - cs);
                if n.muted || t < origin {
                    continue;
                }
                let mut note = *n;
                note.tick = t - origin;
                note.length = n.length.min(e - t).max(1);
                notes.push(note);
            }
            if let Some(data) = &clip.midi.tab {
                let piece = data.window(cs, ce, s - origin);
                match &mut tab {
                    Some(acc) => acc.append(piece),
                    None => tab = Some(piece),
                }
            }
        }
    }
    if notes.is_empty() {
        return None;
    }
    notes.sort_by_key(|n| (n.tick, n.key));
    notes.dedup_by_key(|n| (n.tick, n.key));
    if tab.is_none() && is_drum_name(name) {
        let mut t = TabTrack::new(name, "", atomik_tab::Tuning::from_opens(&[0], 0));
        t.is_percussion = true;
        t.midi_program = 0;
        t.notes = notes
            .iter()
            .map(|n| {
                let mut tn = TabNote::new(n.tick, n.length, 0, n.key);
                tn.velocity = (n.velocity * 127.0).round().clamp(1.0, 127.0) as u8;
                tn
            })
            .collect();
        return Some(t);
    }
    let midi = MidiData {
        tab: tab.clone(),
        ..MidiData::default()
    };
    let edit: Result<TabEdit, _> = if tab.is_some() {
        tab_edit::auto_fret_clip(&notes, &midi, &FretOptions::default())
    } else {
        tab_edit::start_tab(&notes, &midi, instrument_for_name(name), true)
    };
    let edit = match edit {
        Ok(e) => e,
        Err(e) => {
            warnings.push(format!("{name}: left out ({e})"));
            return None;
        }
    };
    let data = edit.midi.tab.as_ref()?;
    let mut out = TabTrack::new(name, &data.instrument_id, data.tuning.clone());
    out.capo = data.capo;
    out.midi_program = match data.instrument_id.as_str() {
        id if id.starts_with("bass") => 33,
        id if id.starts_with("banjo") => 105,
        id if id.starts_with("ukulele") => 24,
        _ => 25,
    };
    for n in &edit.notes {
        if edit.unplayable.contains(&(n.tick, n.key)) {
            continue;
        }
        let Some(entry) = data.note_of(n.tick, n.key) else {
            continue;
        };
        let (velocity, length) = entry
            .plain
            .map_or((n.velocity, n.length), |p| (p.velocity, p.length));
        let mut tn = TabNote::new(n.tick, length.max(1), entry.string, entry.fret);
        tn.velocity = (velocity * 127.0).round().clamp(1.0, 127.0) as u8;
        tn.tech = entry.tech.clone();
        out.notes.push(tn);
    }
    if !edit.unplayable.is_empty() {
        warnings.push(format!(
            "{name}: {} note{} no string of the tuning can play left out",
            edit.unplayable.len(),
            if edit.unplayable.len() == 1 { "" } else { "s" }
        ));
    }
    out.diagrams = data
        .diagrams
        .iter()
        .map(|d| DiagramRef {
            tick: d.tick,
            name: d.name.clone(),
            voicing: d.voicing.clone(),
        })
        .collect();
    out.lyrics = data
        .lyrics
        .iter()
        .map(|l| (l.tick, l.text.clone()))
        .collect();
    Some(out)
}

#[cfg(test)]
#[path = "tab_file_tests.rs"]
pub(crate) mod tests;
