//! The MusicXML reader: partwise XML into a [`TabScore`].
//!
//! The document is read into per-measure raw notes first (positions in ticks from the start of the
//! measure), then the measure lengths are agreed across the parts, notes without a string and fret
//! are placed by the fretter, repeats are expanded and the notes are emitted on the linear timeline
//! with ties, hammer-ons and pull-offs linked on the way.

use std::collections::BTreeMap;

use super::{MusicXmlError, limits, map_gp};
use crate::chord::ChordSymbol;
use crate::fretting::{FretEvent, FretOptions, NoteSlot, Placement, fret_sequence};
use crate::gp::convert::expand;
use crate::gp::dom::MasterBar;
use crate::gp::xml::{El, Xml};
use crate::gp::{ImportWarning, Warnings};
use crate::instrument::{Capo, MAX_CAPO, MAX_FRETS, StringDef, Tuning, catalogue, find_instrument};
use crate::score::{
    BarInfo, Bend, DiagramRef, Grace, Harmonic, HarmonicKind, KeySignature, PPQ, SlideKind,
    StrumDirection, TabNote, TabScore, TabTrack, TechFlags, Techniques, VibratoKind, bar_ticks,
    limits as score_limits,
};
use crate::voicing::{Barre, Voicing, assign_fingers};

/// The furthest position kept within a measure (the same cap as the Guitar Pro readers).
const MAX_POS: i64 = 1 << 40;

/// The velocity MusicXML calls the default (`dynamics="100"`).
const DEFAULT_VELOCITY: i64 = 90;

fn int(s: &str) -> Option<i64> {
    let t = s.trim();
    t.parse::<i64>().ok().or_else(|| {
        t.parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map(|f| f.round() as i64)
    })
}

fn float(s: &str) -> Option<f64> {
    // A tempo may read "120" or "c. 120" or "120-132": take the first number.
    let t = s.trim();
    let end = t
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*c == '-' && *i == 0)))
        .map_or(t.len(), |(i, _)| i);
    let head = &t[..end];
    if head.is_empty() {
        let digits: String = t
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        return digits.parse::<f64>().ok().filter(|f| f.is_finite());
    }
    head.parse::<f64>().ok().filter(|f| f.is_finite())
}

fn child_int(el: El<'_>, name: &str) -> Option<i64> {
    el.child_text(name).and_then(int)
}

/// Ticks of a duration in `div` divisions per quarter note (rounded).
fn conv(units: i64, div: i64) -> i64 {
    let div = i128::from(div.max(1));
    let v = (i128::from(units) * i128::from(PPQ) + div / 2).div_euclid(div);
    v.clamp(-(1 << 50), 1 << 50) as i64
}

/// A position within a measure: divisions per quarter note may change from one attributes element
/// to the next, so the position is a base in ticks plus an offset in the current divisions.
struct Cursor {
    div: i64,
    base: i64,
    pos: i64,
}

impl Cursor {
    fn new(div: i64) -> Self {
        Self {
            div,
            base: 0,
            pos: 0,
        }
    }

    fn tick(&self) -> i64 {
        self.base
            .saturating_add(conv(self.pos, self.div))
            .clamp(0, MAX_POS)
    }

    fn advance(&mut self, units: i64) {
        self.pos = self.pos.saturating_add(units);
    }

    fn set_div(&mut self, div: i64) {
        self.base = self.tick();
        self.pos = 0;
        self.div = div;
    }
}

#[derive(Default)]
struct PartInfo {
    id: String,
    name: String,
    program: Option<u8>,
    channel: Option<u8>,
    /// `(instrument id, unpitched key)` in the order the part lists them.
    instruments: Vec<(String, Option<u8>)>,
}

#[derive(Clone)]
struct RawGrace {
    ticks: i64,
    on_beat: bool,
    /// The fret when the file gives one; else the fret is looked up from `pitch`.
    fret: Option<u8>,
    pitch: Option<u8>,
    string: Option<u32>,
}

struct RawNote {
    off: i64,
    len: i64,
    /// The sounding MIDI key (after any transposition), when the note is pitched.
    pitch: Option<i32>,
    /// MusicXML string number (1 = the highest string).
    string: Option<u32>,
    fret: Option<i64>,
    unpitched: Option<u8>,
    instrument: Option<String>,
    is_unpitched: bool,
    velocity: u8,
    tech: Techniques,
    tie_stop: bool,
    hammer_stop: bool,
    pull_stop: bool,
    hammer_start: bool,
    pull_start: bool,
    grace: Option<RawGrace>,
    is_grace: bool,
    lyric: Option<(i64, String, String)>,
    staff: u32,
    placed: Option<Placement>,
    dropped: bool,
}

struct FrameNote {
    string: u32,
    fret: i64,
    finger: Option<u8>,
    barre: Option<bool>,
}

struct Frame {
    first_fret: i64,
    notes: Vec<FrameNote>,
}

struct Harmony {
    off: i64,
    chord: Option<ChordSymbol>,
    name: String,
    frame: Option<Frame>,
}

#[derive(Default)]
struct Measure {
    extent: i64,
    implicit: bool,
    notes: Vec<RawNote>,
    harmonies: Vec<Harmony>,
    tempo: Vec<(i64, f64)>,
    rehearsal: Vec<(i64, String)>,
    time: Option<(u8, u8)>,
    key: Option<KeySignature>,
    repeat_forward: bool,
    repeat_backward: Option<u8>,
    ending_start: Option<u8>,
    ending_stop: bool,
}

#[derive(Default)]
struct Part {
    info: PartInfo,
    measures: Vec<Measure>,
    tab_staff: Option<u32>,
    /// `(line, key)` of every `staff-tuning`, line 1 = the bottom line.
    tunings: Vec<(i64, i32)>,
    staff_lines: Option<i64>,
    capo: u8,
    percussion: bool,
}

/// Document-wide state while a part is read.
struct PartState {
    div: i64,
    velocity: u8,
    transpose: i32,
    palm: bool,
    ring: bool,
    /// Which region a closing `dashes` or `bracket` ends: 1 palm mute, 2 let ring.
    last_region: u8,
}

fn dynamics_velocity(name: &str) -> Option<u8> {
    Some(match name {
        "pppppp" | "ppppp" | "pppp" => 12,
        "ppp" => 20,
        "pp" => 34,
        "p" => 50,
        "mp" => 64,
        "mf" => 80,
        "f" => 96,
        "ff" => 112,
        "fff" | "ffff" | "fffff" | "ffffff" => 127,
        "fp" => 96,
        "sf" | "sfz" | "sffz" | "sfp" | "sfpp" | "rf" | "rfz" | "fz" => 110,
        _ => return None,
    })
}

fn step_semitone(step: &str) -> Option<i32> {
    Some(match step.trim() {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    })
}

fn pitch_key(p: El<'_>, step: &str, alter: &str, octave: &str) -> Option<i32> {
    let semi = step_semitone(p.child_text(step)?)?;
    let alter = p
        .child_text(alter)
        .and_then(float)
        .map_or(0, |a| a.round() as i32);
    let oct = p.child_text(octave).and_then(int)?;
    let key = (i32::try_from(oct).ok()? + 1) * 12 + semi + alter;
    (-200..=400).contains(&key).then_some(key)
}

// ---------------------------------------------------------------- document level

pub(super) fn read(bytes: &[u8]) -> Result<(TabScore, Vec<ImportWarning>), MusicXmlError> {
    let xml = Xml::parse(bytes).map_err(map_gp)?;
    let root = xml.root();
    match root.name() {
        "score-partwise" => {}
        "score-timewise" => return Err(MusicXmlError::Timewise),
        _ => return Err(MusicXmlError::UnknownFormat),
    }
    let mut w = Warnings::default();
    let mut score = TabScore::new();
    let misc = read_header(root, &mut score);
    let infos = read_part_list(root);

    let mut parts: Vec<Part> = Vec::new();
    let mut budget = score_limits::MAX_NOTES;
    for (i, el) in root.all("part").enumerate() {
        if i >= score_limits::MAX_TRACKS {
            return Err(MusicXmlError::TooMany("parts"));
        }
        let id = el.attr("id").unwrap_or("");
        let info = infos
            .iter()
            .find(|p| p.id == id)
            .map(|p| PartInfo {
                id: p.id.clone(),
                name: p.name.clone(),
                program: p.program,
                channel: p.channel,
                instruments: p.instruments.clone(),
            })
            .unwrap_or_else(|| PartInfo {
                id: id.to_owned(),
                name: format!("Part {}", i + 1),
                ..PartInfo::default()
            });
        parts.push(read_part(el, info, &mut budget, &mut w)?);
    }
    if parts.is_empty() {
        return Err(MusicXmlError::NoMusic);
    }
    // A part without a single note is dropped, unless it is all there is.
    if parts.iter().any(part_has_notes) {
        let before = parts.len();
        parts.retain(part_has_notes);
        if parts.len() < before {
            w.warn("parts without any note were skipped");
        }
    }

    let nbars = parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    let layout = Layout::build(&parts, nbars, &mut w);
    let mut tracks = Vec::new();
    for p in &mut parts {
        tracks.push(prepare_track(p, &layout, &misc, &mut w));
    }
    emit(&parts, &layout, &mut tracks, &mut score, &mut w);
    score.tracks = tracks;
    for msg in score.sanitize() {
        w.warn(&msg);
    }
    Ok((score, w.finish()))
}

fn part_has_notes(p: &Part) -> bool {
    p.measures
        .iter()
        .any(|m| m.notes.iter().any(|n| !n.is_grace))
}

fn read_header(root: El<'_>, score: &mut TabScore) -> BTreeMap<String, String> {
    if let Some(work) = root.child("work") {
        score.title = work.child_text("work-title").unwrap_or("").to_owned();
    }
    let movement = root.child_text("movement-title").unwrap_or("").to_owned();
    if score.title.is_empty() {
        score.title = movement;
    } else {
        score.subtitle = movement;
    }
    let mut misc = BTreeMap::new();
    if let Some(id) = root.child("identification") {
        for c in id.all("creator") {
            let text = c.text().to_owned();
            match c.attr("type").unwrap_or("").to_ascii_lowercase().as_str() {
                "composer" => score.music_by = text,
                "lyricist" => score.words_by = text,
                "arranger" | "transcriber" => score.tab_by = text,
                "artist" | "performer" => score.artist = text,
                _ => {}
            }
        }
        if let Some(r) = id.child_text("rights") {
            score.copyright = r.to_owned();
        }
        if let Some(m) = id.child("miscellaneous") {
            for f in m.all("miscellaneous-field") {
                if let Some(name) = f.attr("name") {
                    misc.insert(name.to_owned(), f.text().to_owned());
                }
            }
        }
    }
    if let Some(a) = misc.get("atomik-album") {
        score.album = a.clone();
    }
    if let Some(a) = misc.get("atomik-notices") {
        score.notices = a.clone();
    }
    if score.title.is_empty()
        && let Some(credit) = root.child("credit")
        && let Some(words) = credit.child_text("credit-words")
    {
        score.title = words.to_owned();
    }
    misc
}

fn read_part_list(root: El<'_>) -> Vec<PartInfo> {
    let mut out = Vec::new();
    let Some(list) = root.child("part-list") else {
        return out;
    };
    for sp in list.all("score-part") {
        let mut info = PartInfo {
            id: sp.attr("id").unwrap_or("").to_owned(),
            name: sp
                .child_text("part-name")
                .filter(|s| !s.is_empty())
                .or_else(|| sp.child_text("part-abbreviation").filter(|s| !s.is_empty()))
                .unwrap_or("")
                .to_owned(),
            ..PartInfo::default()
        };
        let mut instrument_names = Vec::new();
        for si in sp.all("score-instrument") {
            if let Some(n) = si.child_text("instrument-name") {
                instrument_names.push(n.to_owned());
            }
        }
        if info.name.is_empty()
            && let Some(n) = instrument_names.first()
        {
            info.name = n.clone();
        }
        for mi in sp.all("midi-instrument") {
            let id = mi.attr("id").unwrap_or("").to_owned();
            let unpitched = child_int(mi, "midi-unpitched")
                .and_then(|u| u8::try_from(u - 1).ok())
                .filter(|u| *u <= 127);
            if info.channel.is_none() {
                info.channel = child_int(mi, "midi-channel").and_then(|c| u8::try_from(c).ok());
            }
            if info.program.is_none() {
                info.program = child_int(mi, "midi-program").and_then(|p| u8::try_from(p - 1).ok());
            }
            info.instruments.push((id, unpitched));
        }
        out.push(info);
    }
    out
}

// ---------------------------------------------------------------- one part

fn read_part(
    el: El<'_>,
    info: PartInfo,
    budget: &mut usize,
    w: &mut Warnings,
) -> Result<Part, MusicXmlError> {
    let mut part = Part {
        info,
        ..Part::default()
    };
    let mut st = PartState {
        div: 1,
        velocity: DEFAULT_VELOCITY as u8,
        transpose: 0,
        palm: false,
        ring: false,
        last_region: 0,
    };
    let mut cue_warned = false;
    for (mi, m) in el.all("measure").enumerate() {
        if mi >= score_limits::MAX_BARS {
            return Err(MusicXmlError::TooMany("bars"));
        }
        let mut mr = Measure {
            implicit: m.attr("implicit") == Some("yes"),
            ..Measure::default()
        };
        let mut cur = Cursor::new(st.div);
        let mut last_start = 0i64;
        for c in m.children() {
            match c.name() {
                "attributes" => read_attributes(c, &mut st, &mut cur, &mut part, &mut mr, w),
                "note" => {
                    if c.has("cue") {
                        if !cue_warned {
                            w.warn("cue notes were skipped");
                            cue_warned = true;
                        }
                        skip_note(c, &mut cur, &mut last_start, &mut mr);
                    } else {
                        read_note(c, &mut cur, &mut last_start, &mut st, &mut mr, budget, w)?;
                    }
                }
                "backup" => {
                    let d = child_int(c, "duration")
                        .unwrap_or(0)
                        .clamp(0, limits::MAX_DIVISIONS * 1000);
                    cur.advance(-d);
                }
                "forward" => {
                    let d = child_int(c, "duration")
                        .unwrap_or(0)
                        .clamp(0, limits::MAX_DIVISIONS * 1000);
                    cur.advance(d);
                    mr.extent = mr.extent.max(cur.tick());
                }
                "direction" => read_direction(c, &cur, &mut st, &mut mr, w),
                "sound" => read_sound(c, &cur, &mut st, &mut mr, w),
                "harmony" => read_harmony(c, &cur, &mut mr, w),
                "barline" => read_barline(c, &mut mr),
                _ => {}
            }
        }
        mr.extent = mr.extent.max(cur.tick());
        attach_graces(&mut mr);
        part.measures.push(mr);
    }
    part.percussion = part
        .measures
        .iter()
        .any(|m| m.notes.iter().any(|n| n.is_unpitched))
        || (part.info.channel == Some(10) && part.info.instruments.iter().any(|i| i.1.is_some()));
    // Keep only the tab staff of a part that has both a notation staff and a tab staff.
    if let Some(ts) = part.tab_staff {
        for m in &mut part.measures {
            m.notes.retain(|n| n.staff == ts);
        }
    }
    Ok(part)
}

/// Hands each grace note of a measure to the next main note on its string (any string when the
/// grace has none), and takes the grace note itself out of the measure's notes.
fn attach_graces(mr: &mut Measure) {
    let n = mr.notes.len();
    for gi in 0..n {
        if !mr.notes[gi].is_grace {
            continue;
        }
        let Some(g) = mr.notes[gi].grace.clone() else {
            mr.notes[gi].dropped = true;
            continue;
        };
        let target = (gi + 1..n).find(|&k| {
            !mr.notes[k].is_grace && (g.string.is_none() || mr.notes[k].string == g.string)
        });
        if let Some(k) = target
            && mr.notes[k].grace.is_none()
        {
            mr.notes[k].grace = Some(g);
        }
        mr.notes[gi].dropped = true;
    }
}

fn skip_note(n: El<'_>, cur: &mut Cursor, last_start: &mut i64, mr: &mut Measure) {
    if n.has("grace") {
        return;
    }
    let d = child_int(n, "duration").unwrap_or(0).max(0);
    if !n.has("chord") {
        *last_start = cur.tick();
        cur.advance(d);
        mr.extent = mr.extent.max(cur.tick());
    }
}

fn read_attributes(
    a: El<'_>,
    st: &mut PartState,
    cur: &mut Cursor,
    part: &mut Part,
    mr: &mut Measure,
    w: &mut Warnings,
) {
    if let Some(d) = child_int(a, "divisions") {
        if (1..=limits::MAX_DIVISIONS).contains(&d) {
            st.div = d;
            cur.set_div(d);
        } else {
            w.warn("a divisions value outside the supported range was ignored");
        }
    }
    if let Some(k) = a.child("key")
        && let Some(f) = child_int(k, "fifths")
    {
        let minor = k
            .child_text("mode")
            .is_some_and(|m| m.eq_ignore_ascii_case("minor") || m.eq_ignore_ascii_case("aeolian"));
        mr.key = Some(KeySignature {
            fifths: f.clamp(-7, 7) as i8,
            minor,
        });
    }
    if let Some(t) = a.child("time")
        && let (Some(b), Some(bt)) = (t.child_text("beats"), t.child_text("beat-type"))
    {
        let num: i64 = b.split('+').filter_map(int).sum();
        let den = bt.split('+').next().and_then(int).unwrap_or(0);
        match (u8::try_from(num), u8::try_from(den)) {
            (Ok(n), Ok(d)) if n > 0 && d > 0 && d.is_power_of_two() => mr.time = Some((n, d)),
            _ => w.warn("a time signature this reader cannot use was ignored"),
        }
    }
    for c in a.all("clef") {
        if c.child_text("sign")
            .is_some_and(|s| s.eq_ignore_ascii_case("TAB"))
        {
            let n = c.attr("number").and_then(int).unwrap_or(1).clamp(1, 32) as u32;
            part.tab_staff.get_or_insert(n);
        }
    }
    if let Some(tr) = a.child("transpose") {
        let chromatic = child_int(tr, "chromatic").unwrap_or(0).clamp(-48, 48) as i32;
        let octave = child_int(tr, "octave-change").unwrap_or(0).clamp(-4, 4) as i32;
        st.transpose = chromatic + 12 * octave;
    }
    for sd in a.all("staff-details") {
        let number = sd.attr("number").and_then(int);
        let wanted = match (number, part.tab_staff) {
            (Some(n), Some(t)) => n == i64::from(t),
            _ => true,
        };
        if !wanted || !part.tunings.is_empty() {
            continue;
        }
        part.staff_lines = child_int(sd, "staff-lines");
        for t in sd.all("staff-tuning") {
            let line = t.attr("line").and_then(int).unwrap_or(0);
            let Some(semi) = t.child_text("tuning-step").and_then(step_semitone) else {
                continue;
            };
            let alter = t
                .child_text("tuning-alter")
                .and_then(float)
                .map_or(0, |a| a.round() as i32);
            let Some(oct) = child_int(t, "tuning-octave") else {
                continue;
            };
            let key = (oct.clamp(-1, 10) as i32 + 1) * 12 + semi + alter;
            if (0..=127).contains(&key) && part.tunings.len() < 64 {
                part.tunings.push((line, key));
            }
        }
        if let Some(c) = child_int(sd, "capo") {
            part.capo = c.clamp(0, i64::from(MAX_CAPO)) as u8;
        }
    }
}

// ---------------------------------------------------------------- notes

fn read_note(
    n: El<'_>,
    cur: &mut Cursor,
    last_start: &mut i64,
    st: &mut PartState,
    mr: &mut Measure,
    budget: &mut usize,
    w: &mut Warnings,
) -> Result<(), MusicXmlError> {
    let is_grace = n.has("grace");
    let is_chord = n.has("chord");
    let is_rest = n.has("rest");
    let dur = if is_grace {
        0
    } else {
        child_int(n, "duration")
            .unwrap_or(0)
            .clamp(0, limits::MAX_DIVISIONS * 1000)
    };
    let start = if is_chord { *last_start } else { cur.tick() };
    let len = if dur > 0 {
        conv(dur, cur.div).max(1)
    } else {
        0
    };
    if !is_chord && !is_grace {
        *last_start = start;
        cur.advance(dur);
        mr.extent = mr.extent.max(cur.tick());
    }
    mr.extent = mr.extent.max(start + len);
    if is_rest {
        return Ok(());
    }
    if *budget == 0 {
        return Err(MusicXmlError::TooMany("notes"));
    }
    *budget -= 1;

    let mut raw = RawNote {
        off: start,
        len,
        pitch: None,
        string: None,
        fret: None,
        unpitched: None,
        instrument: n
            .child("instrument")
            .and_then(|i| i.attr("id"))
            .map(str::to_owned),
        is_unpitched: false,
        velocity: st.velocity,
        tech: Techniques::default(),
        tie_stop: false,
        hammer_stop: false,
        pull_stop: false,
        hammer_start: false,
        pull_start: false,
        grace: None,
        is_grace,
        lyric: None,
        staff: n
            .child_text("staff")
            .and_then(int)
            .unwrap_or(1)
            .clamp(1, 32) as u32,
        placed: None,
        dropped: false,
    };
    if let Some(p) = n.child("pitch") {
        raw.pitch = pitch_key(p, "step", "alter", "octave").map(|k| k + st.transpose);
    }
    if let Some(u) = n.child("unpitched") {
        raw.is_unpitched = true;
        raw.unpitched = pitch_key(u, "display-step", "display-alter", "display-octave")
            .and_then(|k| u8::try_from(k).ok());
    }
    if let Some(d) = n.attr("dynamics").and_then(float) {
        raw.velocity = (d * DEFAULT_VELOCITY as f64 / 100.0)
            .round()
            .clamp(1.0, 127.0) as u8;
    }
    if st.palm {
        raw.tech.flags.insert(TechFlags::PALM_MUTE);
    }
    if st.ring {
        raw.tech.flags.insert(TechFlags::LET_RING);
    }
    if let Some(nh) = n.child("notehead") {
        let t = nh.text().to_ascii_lowercase();
        if t == "x" || t == "cross" {
            raw.tech.flags.insert(TechFlags::DEAD);
        }
        if nh.attr("parentheses") == Some("yes") {
            raw.tech.flags.insert(TechFlags::GHOST);
        }
    }
    for t in n.all("tie") {
        if t.attr("type") == Some("stop") {
            raw.tie_stop = true;
        }
    }
    if let Some(p) = n.child("play")
        && let Some(m) = p.child_text("mute")
        && m.eq_ignore_ascii_case("palm")
    {
        raw.tech.flags.insert(TechFlags::PALM_MUTE);
    }
    if let Some(g) = n.child("grace") {
        raw.grace = Some(RawGrace {
            ticks: grace_ticks(n.child_text("type")),
            on_beat: g.attr("steal-time-following").is_some(),
            fret: None,
            pitch: raw.pitch.and_then(|k| u8::try_from(k).ok()),
            string: None,
        });
    }
    // Lyrics: keep the first verse (the smallest number).
    for l in n.all("lyric") {
        let number = l.attr("number").and_then(int).unwrap_or(1);
        let text = l.child_text("text").unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let syl = l.child_text("syllabic").unwrap_or("single").to_owned();
        if raw.lyric.as_ref().is_none_or(|(num, _, _)| number < *num) {
            raw.lyric = Some((number, syl, text.to_owned()));
        }
    }
    if let Some(nt) = n.child("notations") {
        read_notations(nt, &mut raw, w);
    }
    // A grace note keeps what it knows about its own string and fret.
    if let Some(g) = &mut raw.grace {
        g.string = raw.string;
        g.fret = raw.fret.and_then(|f| u8::try_from(f).ok());
    }
    mr.notes.push(raw);
    Ok(())
}

fn grace_ticks(ty: Option<&str>) -> i64 {
    match ty.unwrap_or("") {
        "quarter" => PPQ,
        "eighth" => PPQ / 2,
        "16th" => PPQ / 4,
        "32nd" => PPQ / 8,
        "64th" => PPQ / 16,
        "128th" => PPQ / 32,
        _ => PPQ / 8,
    }
}

fn read_notations(nt: El<'_>, raw: &mut RawNote, w: &mut Warnings) {
    let mut bends: Vec<(f64, bool, bool, bool)> = Vec::new();
    let mut harmonic: Option<Harmonic> = None;
    let mut harmonic_override: Option<Harmonic> = None;
    for c in nt.children() {
        match c.name() {
            "technical" => {
                for t in c.children() {
                    match t.name() {
                        "string" => {
                            raw.string = t
                                .text()
                                .parse::<u32>()
                                .ok()
                                .filter(|s| (1..=64).contains(s));
                        }
                        "fret" => raw.fret = int(t.text()).filter(|f| (0..=255).contains(f)),
                        "hammer-on" => match t.attr("type") {
                            Some("start") => raw.hammer_start = true,
                            Some("stop") => raw.hammer_stop = true,
                            _ => {}
                        },
                        "pull-off" => match t.attr("type") {
                            Some("start") => raw.pull_start = true,
                            Some("stop") => raw.pull_stop = true,
                            _ => {}
                        },
                        "bend" => bends.push((
                            t.child_text("bend-alter").and_then(float).unwrap_or(0.0),
                            t.has("pre-bend"),
                            t.has("release"),
                            t.has("with-bar"),
                        )),
                        "harmonic" => {
                            let kind = if t.has("artificial") {
                                HarmonicKind::Artificial
                            } else {
                                HarmonicKind::Natural
                            };
                            harmonic = Some(Harmonic { kind, fret: None });
                        }
                        "tap" => raw.tech.flags.insert(TechFlags::TAP),
                        "pluck" => {
                            if t.text().trim() == "p" {
                                raw.tech.flags.insert(TechFlags::THUMB);
                            }
                        }
                        "other-technical" => {
                            other_technical(t.text(), raw, &mut harmonic_override);
                        }
                        _ => {}
                    }
                }
            }
            "articulations" => {
                for t in c.children() {
                    match t.name() {
                        "accent" => raw.tech.flags.insert(TechFlags::ACCENT),
                        "strong-accent" => raw.tech.flags.insert(TechFlags::HEAVY_ACCENT),
                        "staccato" | "staccatissimo" => raw.tech.flags.insert(TechFlags::STACCATO),
                        "scoop" => raw.tech.slide = Some(SlideKind::InFromBelow),
                        "plop" => raw.tech.slide = Some(SlideKind::InFromAbove),
                        "doit" => raw.tech.slide = Some(SlideKind::OutUp),
                        "falloff" => raw.tech.slide = Some(SlideKind::OutDown),
                        _ => {}
                    }
                }
            }
            "ornaments" => {
                for t in c.children() {
                    match t.name() {
                        "tremolo" => {
                            let marks = t.text().trim().parse::<u8>().unwrap_or(0);
                            if matches!(t.attr("type"), None | Some("single"))
                                && (1..=4).contains(&marks)
                            {
                                raw.tech.tremolo_division = 4 << marks;
                            }
                        }
                        "wavy-line" => {
                            if raw.tech.vibrato.is_none() && t.attr("type") == Some("start") {
                                raw.tech.vibrato = Some(VibratoKind::Normal);
                            }
                        }
                        "trill-mark" | "turn" | "inverted-turn" | "mordent"
                        | "inverted-mordent" | "delayed-turn" | "shake" => {
                            w.warn("ornaments (trills, turns, mordents) were ignored")
                        }
                        _ => {}
                    }
                }
            }
            "slide" => {
                if c.attr("type") == Some("start") {
                    raw.tech.slide = Some(SlideKind::Legato);
                }
            }
            "glissando" => {
                if c.attr("type") == Some("start") {
                    raw.tech.slide = Some(SlideKind::Shift);
                }
            }
            "arpeggiate" => match c.attr("direction") {
                Some("up") => raw.tech.strum = Some(StrumDirection::Up),
                Some("down") => raw.tech.strum = Some(StrumDirection::Down),
                _ => {}
            },
            "tied" if c.attr("type") == Some("stop") => raw.tie_stop = true,
            _ => {}
        }
    }
    raw.tech.harmonic = harmonic_override.or(harmonic);
    apply_bends(&bends, raw);
}

fn other_technical(text: &str, raw: &mut RawNote, harmonic: &mut Option<Harmonic>) {
    let t = text.trim().to_ascii_lowercase();
    match t.as_str() {
        "let ring" | "let-ring" | "l.r." => raw.tech.flags.insert(TechFlags::LET_RING),
        "palm mute" | "palm-mute" | "p.m." => raw.tech.flags.insert(TechFlags::PALM_MUTE),
        "slap" => raw.tech.flags.insert(TechFlags::SLAP),
        "pop" => raw.tech.flags.insert(TechFlags::POP),
        "fade in" | "fade-in" => raw.tech.flags.insert(TechFlags::FADE_IN),
        "vibrato" => raw.tech.vibrato = Some(VibratoKind::Normal),
        "vibrato wide" | "wide vibrato" | "vibrato-wide" => {
            raw.tech.vibrato = Some(VibratoKind::Wide)
        }
        _ => {
            if let Some(rest) = t.strip_prefix("harmonic:") {
                let mut it = rest.split(':');
                let kind = match it.next().unwrap_or("") {
                    "natural" => HarmonicKind::Natural,
                    "artificial" => HarmonicKind::Artificial,
                    "pinch" => HarmonicKind::Pinch,
                    "tapped" => HarmonicKind::Tapped,
                    "semi" => HarmonicKind::Semi,
                    _ => return,
                };
                let fret = it
                    .next()
                    .and_then(|f| f.parse::<f32>().ok())
                    .filter(|f| f.is_finite() && (0.0..=48.0).contains(f));
                *harmonic = Some(Harmonic { kind, fret });
            }
        }
    }
}

/// Builds the bend or whammy curve from the `bend` elements of a note.
fn apply_bends(bends: &[(f64, bool, bool, bool)], raw: &mut RawNote) {
    let cents = |semis: f64| (semis * 100.0).round().clamp(-4800.0, 4800.0) as i32;
    let bar: Vec<_> = bends.iter().filter(|b| b.3).collect();
    if let Some(b) = bar.first() {
        raw.tech.whammy = Some(Bend::bend_release(cents(b.0)));
    }
    let plain: Vec<_> = bends.iter().filter(|b| !b.3).collect();
    let Some(first) = plain.first() else {
        return;
    };
    let release = plain.iter().find(|b| b.2).copied();
    let main = plain.iter().find(|b| !b.2).copied();
    let curve = match (main, release) {
        (Some(m), Some(_)) => {
            let c = cents(m.0);
            if m.1 {
                Bend::pre_bend_release(c)
            } else {
                Bend::bend_release(c)
            }
        }
        (Some(m), None) => {
            let c = cents(m.0);
            if m.1 { Bend::pre_bend(c) } else { Bend::up(c) }
        }
        // A lone release: the note starts bent and comes back.
        (None, Some(r)) => Bend::pre_bend_release(cents(r.0)),
        (None, None) => Bend::up(cents(first.0)),
    };
    if curve.peak() != 0 {
        raw.tech.bend = Some(curve);
    }
}

// ---------------------------------------------------------------- directions

fn metronome_bpm(m: El<'_>) -> Option<f64> {
    let per_minute = m.child_text("per-minute").and_then(float)?;
    let unit = match m.child_text("beat-unit").unwrap_or("quarter") {
        "breve" => 8.0,
        "whole" => 4.0,
        "half" => 2.0,
        "quarter" => 1.0,
        "eighth" => 0.5,
        "16th" => 0.25,
        "32nd" => 0.125,
        "64th" => 0.0625,
        _ => 1.0,
    };
    let mut ratio = unit;
    let mut add = unit / 2.0;
    for _ in m.all("beat-unit-dot") {
        ratio += add;
        add /= 2.0;
    }
    Some(per_minute * ratio)
}

fn read_direction(d: El<'_>, cur: &Cursor, st: &mut PartState, mr: &mut Measure, w: &mut Warnings) {
    let offset = child_int(d, "offset").unwrap_or(0);
    let pos = (cur.tick() + conv(offset, cur.div)).max(0);
    let mut metronome = None;
    let mut sound_tempo = None;
    if let Some(s) = d.child("sound") {
        sound_tempo = s.attr("tempo").and_then(float);
        read_sound_jumps(s, w);
        apply_sound_dynamics(s, st);
    }
    for dt in d.all("direction-type") {
        for c in dt.children() {
            match c.name() {
                "metronome" => metronome = metronome_bpm(c),
                "rehearsal" => {
                    if !c.text().is_empty() {
                        mr.rehearsal.push((pos, c.text().to_owned()));
                    }
                }
                "dynamics" => {
                    if let Some(v) = c.children().find_map(|x| dynamics_velocity(x.name())) {
                        st.velocity = v;
                    }
                }
                "words" => {
                    let t = c.text().trim().to_ascii_lowercase();
                    if t.starts_with("p.m") || t == "pm" || t.starts_with("palm mute") {
                        st.palm = true;
                        st.last_region = 1;
                    } else if t.starts_with("let ring") || t.starts_with("l.r") {
                        st.ring = true;
                        st.last_region = 2;
                    }
                }
                "dashes" | "bracket" if c.attr("type") == Some("stop") => match st.last_region {
                    1 => st.palm = false,
                    2 => st.ring = false,
                    _ => {}
                },
                _ => {}
            }
        }
    }
    if let Some(bpm) = sound_tempo.or(metronome)
        && bpm.is_finite()
        && bpm > 0.0
    {
        mr.tempo.push((pos, bpm));
    }
}

fn read_sound(s: El<'_>, cur: &Cursor, st: &mut PartState, mr: &mut Measure, w: &mut Warnings) {
    if let Some(bpm) = s.attr("tempo").and_then(float)
        && bpm.is_finite()
        && bpm > 0.0
    {
        mr.tempo.push((cur.tick(), bpm));
    }
    read_sound_jumps(s, w);
    apply_sound_dynamics(s, st);
}

fn apply_sound_dynamics(s: El<'_>, st: &mut PartState) {
    if let Some(d) = s.attr("dynamics").and_then(float) {
        st.velocity = (d * DEFAULT_VELOCITY as f64 / 100.0)
            .round()
            .clamp(1.0, 127.0) as u8;
    }
}

fn read_sound_jumps(s: El<'_>, w: &mut Warnings) {
    if ["dacapo", "dalsegno", "tocoda", "fine"]
        .iter()
        .any(|a| s.attr(a).is_some())
    {
        w.warn("jumps (D.C., D.S., coda, fine) are not followed");
    }
}

fn read_barline(b: El<'_>, mr: &mut Measure) {
    if let Some(r) = b.child("repeat") {
        match r.attr("direction") {
            Some("forward") => mr.repeat_forward = true,
            Some("backward") => {
                let times = r.attr("times").and_then(int).unwrap_or(2).clamp(2, 16);
                mr.repeat_backward = Some(times as u8);
            }
            _ => {}
        }
    }
    if let Some(e) = b.child("ending") {
        match e.attr("type") {
            Some("start") => {
                let mut mask = 0u8;
                // "1", "1, 2", "1-3": the passes the ending plays on.
                for part in e.attr("number").unwrap_or("1").split(',') {
                    let part = part.trim().trim_end_matches('.');
                    if let Some((a, z)) = part.split_once('-') {
                        if let (Some(a), Some(z)) = (int(a), int(z)) {
                            for p in a.max(1)..=z.min(8) {
                                mask |= 1 << (p - 1);
                            }
                        }
                    } else if let Some(p) = int(part)
                        && (1..=8).contains(&p)
                    {
                        mask |= 1 << (p - 1);
                    }
                }
                mr.ending_start = Some(if mask == 0 { 1 } else { mask });
            }
            Some("stop") | Some("discontinue") => mr.ending_stop = true,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- harmony

fn kind_suffix(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "major" => "",
        "minor" => "m",
        "augmented" => "aug",
        "diminished" => "dim",
        "dominant" => "7",
        "major-seventh" => "maj7",
        "minor-seventh" => "m7",
        "diminished-seventh" => "dim7",
        "augmented-seventh" => "7#5",
        "half-diminished" => "m7b5",
        "major-minor" => "m(maj7)",
        "major-sixth" => "6",
        "minor-sixth" => "m6",
        "dominant-ninth" => "9",
        "major-ninth" => "maj9",
        "minor-ninth" => "m9",
        "dominant-11th" => "11",
        "major-11th" => "maj11",
        "minor-11th" => "m11",
        "dominant-13th" => "13",
        "major-13th" => "maj13",
        "minor-13th" => "m13",
        "suspended-second" => "sus2",
        "suspended-fourth" => "sus4",
        "power" => "5",
        _ => return None,
    })
}

fn note_name(el: El<'_>, step: &str, alter: &str) -> Option<String> {
    let s = el.child_text(step)?.trim();
    let a = el
        .child_text(alter)
        .and_then(float)
        .map_or(0, |a| a.round() as i32);
    let mut out = s.to_owned();
    for _ in 0..a.unsigned_abs().min(2) {
        out.push(if a > 0 { '#' } else { 'b' });
    }
    Some(out)
}

fn read_harmony(h: El<'_>, cur: &Cursor, mr: &mut Measure, w: &mut Warnings) {
    let offset = child_int(h, "offset").unwrap_or(0);
    let off = (cur.tick() + conv(offset, cur.div)).max(0);
    let kind_el = h.child("kind");
    let kind = kind_el.map_or("", |k| k.text().trim());
    let text_attr = kind_el.and_then(|k| k.attr("text"));
    let root = h
        .child("root")
        .and_then(|r| note_name(r, "root-step", "root-alter"));
    let bass = h
        .child("bass")
        .and_then(|b| note_name(b, "bass-step", "bass-alter"));
    let frame = h.child("frame").map(read_frame);
    let mut chord = None;
    let mut name = String::new();
    match root {
        None => {
            // A function or a numeral: not a chord name this reader can use.
            if frame.is_none() {
                w.warn("chord symbols that are not letter names were skipped");
                return;
            }
        }
        Some(root) if kind == "none" && text_attr.is_some_and(|t| !t.is_empty()) => {
            let _ = root;
            name = text_attr.unwrap_or("").to_owned();
        }
        Some(root) => {
            let tail = bass.map(|b| format!("/{b}")).unwrap_or_default();
            let mut candidates = Vec::new();
            if let Some(t) = text_attr {
                candidates.push(format!("{root}{t}{tail}"));
            }
            if let Some(s) = kind_suffix(kind) {
                candidates.push(format!("{root}{s}{}{tail}", degrees(h)));
                candidates.push(format!("{root}{s}{tail}"));
            }
            for c in &candidates {
                if let Ok(sym) = ChordSymbol::parse(c) {
                    name = sym.name();
                    chord = Some(sym);
                    break;
                }
            }
            if chord.is_none() {
                name = candidates
                    .last()
                    .cloned()
                    .unwrap_or_else(|| format!("{root}{tail}"));
                w.warn("a chord symbol this reader cannot parse was kept as a name only");
            }
        }
    }
    if chord.is_none() && name.is_empty() && frame.is_none() {
        return;
    }
    mr.harmonies.push(Harmony {
        off,
        chord,
        name,
        frame,
    });
}

/// `add`ed degrees as chord text (`add9`, `(b9)`).
fn degrees(h: El<'_>) -> String {
    let mut out = String::new();
    for d in h.all("degree") {
        let value = child_int(d, "degree-value").unwrap_or(0);
        let alter = d
            .child_text("degree-alter")
            .and_then(float)
            .map_or(0, |a| a.round() as i32);
        let ty = d.child_text("degree-type").unwrap_or("");
        if ty != "add" || !(2..=13).contains(&value) {
            continue;
        }
        match alter {
            0 => out.push_str(&format!("add{value}")),
            a if a > 0 => out.push_str(&format!("(#{value})")),
            _ => out.push_str(&format!("(b{value})")),
        }
    }
    out
}

fn read_frame(f: El<'_>) -> Frame {
    let mut notes = Vec::new();
    for n in f.all("frame-note") {
        let Some(string) = child_int(n, "string").and_then(|s| u32::try_from(s).ok()) else {
            continue;
        };
        let Some(fret) = child_int(n, "fret") else {
            continue;
        };
        let finger = n
            .child_text("fingering")
            .and_then(|t| t.trim().parse::<u8>().ok())
            .filter(|f| (1..=5).contains(f));
        let barre = n.child("barre").and_then(|b| match b.attr("type") {
            Some("start") => Some(true),
            Some("stop") => Some(false),
            _ => None,
        });
        if notes.len() < 64 {
            notes.push(FrameNote {
                string,
                fret,
                finger,
                barre,
            });
        }
    }
    Frame {
        first_fret: child_int(f, "first-fret").unwrap_or(1),
        notes,
    }
}

// ---------------------------------------------------------------- measure layout

/// What the parts agree on: the length of every measure, their file-order start ticks, the
/// effective meter and the played order of the measures.
struct Layout {
    lens: Vec<i64>,
    meters: Vec<(u8, u8)>,
    plays: Vec<usize>,
    /// The start of every played measure on the unrolled timeline.
    starts: Vec<i64>,
    /// The start of every measure in file order (for placing notes).
    file_starts: Vec<i64>,
    bars: Vec<MasterBar>,
}

impl Layout {
    #[allow(clippy::needless_range_loop)]
    fn build(parts: &[Part], nbars: usize, w: &mut Warnings) -> Layout {
        // The first part that has the information leads, the others fill gaps.
        let mut time: Option<(u8, u8)> = None;
        let mut nominal: Vec<Option<(u8, u8)>> = Vec::with_capacity(nbars);
        for i in 0..nbars {
            if let Some(t) = parts
                .iter()
                .find_map(|p| p.measures.get(i).and_then(|m| m.time))
            {
                time = Some(t);
            }
            nominal.push(time);
        }
        let mut lens = Vec::with_capacity(nbars);
        let mut meters = Vec::with_capacity(nbars);
        for i in 0..nbars {
            let extent = parts
                .iter()
                .filter_map(|p| p.measures.get(i))
                .map(|m| m.extent)
                .max()
                .unwrap_or(0)
                .min(crate::gp::limits::MAX_TICKS);
            let implicit = parts
                .iter()
                .filter_map(|p| p.measures.get(i))
                .any(|m| m.implicit);
            let (len, meter) = match nominal[i] {
                Some((n, d)) => {
                    let nom = bar_ticks(n, d);
                    if implicit || extent > nom {
                        (extent.max(1), fit_meter(extent.max(1), (n, d)))
                    } else {
                        (nom, (n, d))
                    }
                }
                None => (extent.max(1), fit_meter(extent.max(1), (4, 4))),
            };
            if meter == (0, 0) {
                w.warn("a measure has a length that no time signature can express; 4/4 was used");
            }
            lens.push(len);
            meters.push(if meter == (0, 0) { (4, 4) } else { meter });
        }
        let mut file_starts = Vec::with_capacity(nbars);
        let mut t = 0i64;
        for l in &lens {
            file_starts.push(t);
            t += *l;
        }
        // The first part with bars leads the repeat structure.
        let lead = parts.iter().max_by_key(|p| p.measures.len());
        let mut bars: Vec<MasterBar> = Vec::with_capacity(nbars);
        let mut ending_mask = 0u8;
        for i in 0..nbars {
            let (n, d) = meters[i];
            let mut mb = MasterBar::new(n, d);
            if let Some(m) = lead.and_then(|p| p.measures.get(i)) {
                mb.repeat_open = m.repeat_forward;
                mb.repeat_close = m.repeat_backward;
                if let Some(mask) = m.ending_start {
                    ending_mask = mask;
                }
                mb.alt = ending_mask;
                if m.ending_stop {
                    ending_mask = 0;
                }
            }
            bars.push(mb);
        }
        let plays = expand(&bars, w);
        let mut starts = Vec::with_capacity(plays.len());
        let mut t = 0i64;
        for &i in &plays {
            starts.push(t);
            t += lens[i];
        }
        Layout {
            lens,
            meters,
            plays,
            starts,
            file_starts,
            bars,
        }
    }
}

/// The meter of a measure of `len` ticks: `nominal` when it fits, else the same denominator, else
/// any power-of-two denominator that does. `(0, 0)` when none does.
fn fit_meter(len: i64, nominal: (u8, u8)) -> (u8, u8) {
    let fits = |d: u8| -> Option<(u8, u8)> {
        let unit = 4 * PPQ / i64::from(d);
        (len % unit == 0)
            .then(|| u8::try_from(len / unit).ok())
            .flatten()
            .filter(|n| *n > 0)
            .map(|n| (n, d))
    };
    if bar_ticks(nominal.0, nominal.1) == len {
        return nominal;
    }
    fits(nominal.1)
        .or_else(|| [4u8, 8, 16, 32, 64].iter().find_map(|d| fits(*d)))
        .unwrap_or((0, 0))
}

// ---------------------------------------------------------------- tracks

/// The instrument id and tuning from a part's staff details, the private note a Atomik Rack
/// file carries, or a guess.
fn resolve_instrument(
    p: &Part,
    misc: &BTreeMap<String, String>,
    max_string: usize,
    w: &mut Warnings,
) -> (String, Tuning) {
    let mut lines = p.tunings.clone();
    lines.sort_by_key(|l| std::cmp::Reverse(l.0));
    let mut opens: Vec<u8> = lines
        .iter()
        .filter_map(|l| u8::try_from(l.1).ok())
        .collect();
    if opens.len() != lines.len() {
        opens.clear();
    }
    if let Some(n) = p.staff_lines
        && !opens.is_empty()
        && i64::try_from(opens.len()).ok() != Some(n)
    {
        w.warn("a part's staff tuning does not match its number of lines; it was ignored");
        opens.clear();
    }
    if opens.len() > crate::instrument::MAX_STRINGS {
        opens.clear();
    }
    let name = p.info.name.to_ascii_lowercase();
    let program = p.info.program;
    if !opens.is_empty() {
        if let Some(v) = misc.get(&format!("atomik-tuning-{}", p.info.id))
            && let Some((id, t)) = parse_private_tuning(v)
            && t.opens() == opens
        {
            return (id, t);
        }
        for inst in catalogue() {
            for nt in &inst.tunings {
                if nt.tuning.opens() == opens {
                    return (inst.id.to_owned(), nt.tuning.clone());
                }
            }
        }
        let top = opens[0];
        let low = top <= 52;
        let id = match opens.len() {
            1..=3 => "ukulele_soprano",
            4 if low => "bass4",
            4 => "ukulele_soprano",
            5 if name.contains("banjo") => "banjo5",
            5 if low => "bass5",
            6 if low => "bass6",
            5 | 6 => "guitar6",
            7 => "guitar7",
            8 => "guitar8",
            _ => "",
        };
        let mut tuning = Tuning::from_opens(&opens, 24);
        while tuning.validate().is_err() && tuning.frets > 1 {
            tuning.frets -= 1;
        }
        if tuning.validate().is_ok() {
            return (id.to_owned(), tuning);
        }
        w.warn("a part has a tuning outside the MIDI range; a guitar tuning was used");
    }
    // No usable tuning: pick an instrument from the program and the name.
    let id = if name.contains("banjo") {
        "banjo5"
    } else if name.contains("ukulele") || name.contains("uke") {
        "ukulele_concert"
    } else if name.contains("mandolin") {
        "mandolin"
    } else if name.contains("bass") || program.is_some_and(|p| (32..=39).contains(&p)) {
        match max_string {
            0..=4 => "bass4",
            5 => "bass5",
            _ => "bass6",
        }
    } else if max_string >= 8 {
        "guitar8"
    } else if max_string == 7 {
        "guitar7"
    } else {
        "guitar6"
    };
    let inst = find_instrument(id).or_else(|| find_instrument("guitar6"));
    match inst {
        Some(i) => (i.id.to_owned(), i.default_tuning().tuning.clone()),
        None => (
            "guitar6".to_owned(),
            Tuning::from_opens(&[64, 59, 55, 50, 45, 40], 24),
        ),
    }
}

/// The private `miscellaneous-field` the writer adds to keep what `staff-tuning` cannot say:
/// `id=<instrument>;frets=<n>;s=<start>,<course>,<octave 0/1>,<label>|...` in tab order, with
/// the opens in `o=<key>,<key>...`.
fn parse_private_tuning(v: &str) -> Option<(String, Tuning)> {
    let mut id = String::new();
    let mut frets = 0u8;
    let mut opens: Vec<u8> = Vec::new();
    let mut strings: Vec<StringDef> = Vec::new();
    for kv in v.split(';') {
        let (k, val) = kv.split_once('=')?;
        match k.trim() {
            "id" => id = val.trim().to_owned(),
            "frets" => frets = val.trim().parse().ok()?,
            "o" => {
                opens = val
                    .split(',')
                    .map(|o| o.trim().parse::<u8>().ok())
                    .collect::<Option<Vec<_>>>()?;
            }
            "s" => {
                for s in val.split('|') {
                    let mut f = s.split(',');
                    let start: u8 = f.next()?.parse().ok()?;
                    let course: u8 = f.next()?.parse().ok()?;
                    let octave = f.next()? == "1";
                    let label = f.next().unwrap_or("").to_owned();
                    strings.push(StringDef {
                        open: 0,
                        start_fret: start,
                        course,
                        octave_course: octave,
                        label,
                    });
                }
            }
            _ => {}
        }
    }
    if strings.len() != opens.len() || opens.is_empty() || opens.len() > 64 {
        return None;
    }
    for (s, o) in strings.iter_mut().zip(&opens) {
        s.open = *o;
    }
    let tuning = Tuning { strings, frets };
    tuning.validate().ok()?;
    (id.is_empty() || find_instrument(&id).is_some()).then_some((id, tuning))
}

fn prepare_track(
    p: &mut Part,
    layout: &Layout,
    misc: &BTreeMap<String, String>,
    w: &mut Warnings,
) -> TabTrack {
    let name = if p.info.name.is_empty() {
        "Track".to_owned()
    } else {
        p.info.name.clone()
    };
    if p.percussion {
        return prepare_percussion(p, name);
    }
    let max_string = p
        .measures
        .iter()
        .flat_map(|m| m.notes.iter())
        .filter_map(|n| n.string)
        .max()
        .unwrap_or(0) as usize;
    let (id, mut tuning) = resolve_instrument(p, misc, max_string, w);
    if max_string > tuning.len() && p.tunings.is_empty() {
        w.warn("a part uses more strings than its instrument has; notes on missing strings were dropped");
    }
    tuning.frets = tuning.frets.clamp(1, MAX_FRETS);
    let capo = Capo::at(p.capo);
    let mut track = TabTrack::new(&name, &id, tuning);
    track.capo = capo;
    track.midi_program = p
        .info
        .program
        .unwrap_or(if id.starts_with("bass") { 33 } else { 25 })
        .min(127);
    place_notes(p, layout, &mut track, w);
    track
}

fn prepare_percussion(p: &mut Part, name: String) -> TabTrack {
    // Lanes: the order the part lists its instruments in, then new keys as they appear.
    let mut lanes: Vec<u8> = Vec::new();
    let mut by_instrument: BTreeMap<String, u8> = BTreeMap::new();
    for (id, key) in &p.info.instruments {
        if let Some(k) = key {
            by_instrument.insert(id.clone(), *k);
            if !lanes.contains(k) {
                lanes.push(*k);
            }
        }
    }
    for m in &mut p.measures {
        for n in &mut m.notes {
            if !n.is_unpitched {
                n.dropped = true;
                continue;
            }
            let key = n
                .instrument
                .as_ref()
                .and_then(|i| by_instrument.get(i).copied())
                .or(n.unpitched)
                .unwrap_or(38)
                .min(127);
            if !lanes.contains(&key) && lanes.len() < 4096 {
                lanes.push(key);
            }
            let lane = lanes.iter().position(|k| *k == key).unwrap_or(0).min(11);
            n.placed = Some(Placement {
                string: lane,
                fret: key,
            });
        }
    }
    let count = lanes.len().clamp(1, 12);
    let mut track = TabTrack::new(&name, "", Tuning::from_opens(&vec![0; count], 36));
    track.is_percussion = true;
    track.midi_program = 0;
    track
}

/// Gives every pitched note its string and fret: the file's own where it has them, the fretter's
/// choice where it has not, and a tied note's string from the note it continues.
fn place_notes(p: &mut Part, layout: &Layout, track: &mut TabTrack, w: &mut Warnings) {
    let tuning = track.tuning.clone();
    let capo = track.capo;
    let n_strings = tuning.len();
    // `(measure, note)` of every pitched note in time order.
    let mut order: Vec<(i64, usize, usize)> = Vec::new();
    let mut beyond = 0usize;
    for (mi, m) in p.measures.iter_mut().enumerate() {
        for (ni, n) in m.notes.iter_mut().enumerate() {
            if n.dropped {
                continue;
            }
            let key = n.pitch.and_then(|k| u8::try_from(k).ok());
            if let (Some(s), Some(f)) = (n.string, n.fret) {
                let idx = (s - 1) as usize;
                if idx >= n_strings {
                    n.dropped = true;
                    beyond += 1;
                    continue;
                }
                match resolve_fret(&tuning, capo, idx, f, key) {
                    Some(rel) => {
                        n.placed = Some(Placement {
                            string: idx,
                            fret: rel,
                        })
                    }
                    None => {
                        n.dropped = true;
                        beyond += 1;
                        continue;
                    }
                }
            } else if key.is_none() {
                n.dropped = true;
                continue;
            }
            let at = layout.file_starts.get(mi).copied().unwrap_or(0) + n.off;
            order.push((at, mi, ni));
        }
    }
    if beyond > 0 {
        w.warn(&format!(
            "{beyond} notes on strings or frets the tuning does not have were dropped"
        ));
    }
    order.sort_by_key(|o| (o.0, o.1, o.2));
    // Pass A: the free notes (no string and fret, not a tie) as events with the placed notes of
    // the same moment fixed.
    let mut events: Vec<FretEvent> = Vec::new();
    let mut refs: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut i = 0;
    while i < order.len() {
        let at = order[i].0;
        let mut j = i;
        while j < order.len() && order[j].0 == at {
            j += 1;
        }
        let group = &order[i..j];
        let free = group.iter().any(|(_, mi, ni)| {
            let n = &p.measures[*mi].notes[*ni];
            n.placed.is_none() && !n.tie_stop
        });
        if free {
            let mut ev = FretEvent::default();
            let mut rs = Vec::new();
            for &(_, mi, ni) in group {
                let n = &p.measures[mi].notes[ni];
                let Some(key) = n.pitch.and_then(|k| u8::try_from(k).ok()) else {
                    continue;
                };
                if n.placed.is_none() && n.tie_stop {
                    continue;
                }
                ev.notes.push(NoteSlot {
                    key,
                    fixed: n.placed,
                });
                rs.push((mi, ni));
            }
            events.push(ev);
            refs.push(rs);
        }
        i = j;
    }
    if !events.is_empty() {
        let result = fret_sequence(&tuning, capo, &events, None, &FretOptions::default());
        let mut lost = 0usize;
        for (e, rs) in refs.iter().enumerate() {
            for (k, &(mi, ni)) in rs.iter().enumerate() {
                let n = &mut p.measures[mi].notes[ni];
                if n.placed.is_some() {
                    continue;
                }
                match result.placements[e].get(k).copied().flatten() {
                    Some(pl) => n.placed = Some(pl),
                    None => {
                        n.dropped = true;
                        lost += 1;
                    }
                }
            }
        }
        if lost > 0 {
            w.warn(&format!(
                "{lost} notes are out of reach of the part's tuning and were dropped"
            ));
        }
    }
    // Pass B: tied notes without a position take the position of the note they continue.
    let mut last: BTreeMap<u8, (Placement, i64)> = BTreeMap::new();
    let mut lost = 0usize;
    for &(at, mi, ni) in &order {
        let n = &p.measures[mi].notes[ni];
        if n.dropped {
            continue;
        }
        let key = n.pitch.and_then(|k| u8::try_from(k).ok());
        if n.placed.is_none() {
            let inherited = key
                .and_then(|k| last.get(&k))
                .filter(|(_, end)| (end - at).abs() <= 2)
                .map(|(pl, _)| *pl);
            let pl = inherited.or_else(|| {
                key.and_then(|k| {
                    crate::fretting::fret_notes(&tuning, capo, &[k], None, &FretOptions::default())
                        .first()
                        .copied()
                        .flatten()
                })
            });
            let n = &mut p.measures[mi].notes[ni];
            match pl {
                Some(pl) => n.placed = Some(pl),
                None => {
                    n.dropped = true;
                    lost += 1;
                    continue;
                }
            }
        }
        let n = &p.measures[mi].notes[ni];
        if let (Some(pl), Some(k)) = (
            n.placed,
            tuning.pitch(
                n.placed.map_or(0, |p| p.string),
                n.placed.map_or(0, |p| p.fret),
                capo,
            ),
        ) {
            last.insert(k, (pl, at + n.len));
        }
    }
    if lost > 0 {
        w.warn(&format!(
            "{lost} tied notes could not be placed and were dropped"
        ));
    }
    // Grace notes: the grace's own fret, else the fret of its pitch on the main note's string.
    for m in &mut p.measures {
        for n in &mut m.notes {
            if let (Some(g), Some(pl)) = (&mut n.grace, n.placed) {
                if g.fret.is_none() {
                    g.fret = g
                        .pitch
                        .and_then(|k| tuning.fret_for_pitch(pl.string, k, capo));
                }
                let _ = g.string;
            }
        }
    }
}

/// The fret relative to the capo for a note that the file gives a string and a fret: the fret
/// as written when it matches the pitch (or there is no pitch), else the fret counted from the nut
/// when that matches.
fn resolve_fret(
    tuning: &Tuning,
    capo: Capo,
    string: usize,
    fret: i64,
    key: Option<u8>,
) -> Option<u8> {
    let f = u8::try_from(fret).ok()?;
    let raise = capo.raise(string);
    let rel = match key {
        Some(k) if tuning.pitch(string, f, capo) == Some(k) => f,
        Some(k)
            if raise > 0
                && f >= raise
                && tuning
                    .open_pitch(string, Capo::NONE)
                    .is_some_and(|o| i32::from(o) + i32::from(f) == i32::from(k)) =>
        {
            f - raise
        }
        _ => f,
    };
    (rel <= tuning.max_fret(string, capo)).then_some(rel)
}

// ---------------------------------------------------------------- emission

/// What the previous note on a string left behind.
#[derive(Clone, Copy, Default)]
struct Last {
    /// A hammer-on (1) or pull-off (2) that starts at this note and ends at the next on the string.
    pending: u8,
}

fn emit(
    parts: &[Part],
    layout: &Layout,
    tracks: &mut [TabTrack],
    score: &mut TabScore,
    w: &mut Warnings,
) {
    let mut cur_meter = (0u8, 0u8);
    let mut cur_key: Option<KeySignature> = None;
    let mut cur_tempo: Option<f64> = None;
    let mut first_tick: Vec<Option<i64>> = vec![None; layout.lens.len()];
    let mut budget = score_limits::MAX_NOTES;
    let mut seen = vec![false; layout.lens.len()];
    let mut linkers: Vec<Vec<Option<Last>>> = tracks
        .iter()
        .map(|t| vec![None; t.tuning.len().max(1)])
        .collect();
    let mut tempo_at: BTreeMap<i64, f64> = BTreeMap::new();
    let mut marker_at: BTreeMap<i64, String> = BTreeMap::new();
    let mut truncated = false;

    for (play_no, (&mi, &tick0)) in layout.plays.iter().zip(&layout.starts).enumerate() {
        let first = !seen[mi];
        seen[mi] = true;
        if first {
            first_tick[mi] = Some(tick0);
        }
        let meter = layout.meters[mi];
        if meter != cur_meter {
            cur_meter = meter;
            score.meter.push((play_no as u32, meter.0, meter.1));
        }
        let key = parts
            .iter()
            .find_map(|p| p.measures.get(mi).and_then(|m| m.key));
        if let Some(k) = key
            && cur_key != Some(k)
        {
            cur_key = Some(k);
            score.key.push((tick0, k));
        }
        for p in parts {
            if let Some(m) = p.measures.get(mi) {
                for (off, bpm) in &m.tempo {
                    tempo_at.entry(tick0 + off).or_insert(*bpm);
                }
                for (off, name) in &m.rehearsal {
                    marker_at.entry(tick0 + off).or_insert_with(|| name.clone());
                }
            }
        }
        for (ti, p) in parts.iter().enumerate() {
            let Some(m) = p.measures.get(mi) else {
                continue;
            };
            let track = &mut tracks[ti];
            let mut idx: Vec<usize> = (0..m.notes.len()).collect();
            idx.sort_by_key(|&i| (m.notes[i].off, m.notes[i].placed.map_or(0, |p| p.string)));
            for i in idx {
                let n = &m.notes[i];
                if n.dropped {
                    continue;
                }
                let Some(pl) = n.placed else {
                    continue;
                };
                if budget == 0 {
                    truncated = true;
                    break;
                }
                budget -= 1;
                let tick = tick0 + n.off;
                if first && let Some((_, syl, text)) = &n.lyric {
                    let mut t = text.clone();
                    if syl == "begin" || syl == "middle" {
                        t.push('-');
                    }
                    track.lyrics.push((tick, t));
                }
                let mut note = TabNote::new(tick, n.len.max(1), pl.string as u8, pl.fret);
                note.velocity = n.velocity;
                note.tech = n.tech.clone();
                if track.is_percussion {
                    note.tech = Techniques::default();
                    track.notes.push(note);
                    continue;
                }
                if let Some(g) = &n.grace {
                    note.tech.grace = g.fret.map(|f| Grace {
                        fret: f,
                        ticks: g.ticks.max(1),
                        on_beat: g.on_beat,
                    });
                }
                link(&mut linkers[ti], n, &mut note);
                track.notes.push(note);
            }
            for h in &m.harmonies {
                let tick = tick0 + h.off;
                if let Some(c) = &h.chord {
                    track.chords.push((tick, c.clone()));
                }
                if let Some(f) = &h.frame
                    && let Some(v) = voicing_of(f, track.tuning.len())
                {
                    track.diagrams.push(DiagramRef {
                        tick,
                        name: h.name.clone(),
                        voicing: v,
                    });
                }
            }
        }
    }
    if truncated {
        w.warn("the score has too many notes; the rest were dropped");
    }
    // Tempo changes: drop the ones that restate the tempo already in force.
    for (tick, bpm) in tempo_at {
        if cur_tempo != Some(bpm) {
            cur_tempo = Some(bpm);
            score.tempo.push((tick, bpm));
        }
    }
    score.markers = marker_at.into_iter().collect();
    // The original bar structure, one entry per bar of the file.
    for (i, mb) in layout.bars.iter().enumerate() {
        let len = layout.lens[i];
        score.bars.push(BarInfo {
            tick: first_tick[i].unwrap_or(0),
            len,
            repeat_open: mb.repeat_open,
            repeat_close: mb.repeat_close,
            volta: (mb.alt != 0).then_some(mb.alt),
        });
    }
    if score.bars.len() > score_limits::MAX_BARS {
        score.bars.truncate(score_limits::MAX_BARS);
    }
}

/// Applies ties and hammer-on / pull-off links for one note on its string.
fn link(last: &mut [Option<Last>], n: &RawNote, note: &mut TabNote) {
    let Some(slot) = last.get_mut(usize::from(note.string)) else {
        return;
    };
    let prev = *slot;
    let own = if n.hammer_start {
        1
    } else if n.pull_start {
        2
    } else {
        0
    };
    let pending = if n.tie_stop {
        // A tie continues the note before it on the string; with nothing before it, it is plain.
        match prev {
            Some(p) => {
                note.tie = true;
                if own != 0 { own } else { p.pending }
            }
            None => own,
        }
    } else {
        let dest = if n.hammer_stop {
            1
        } else if n.pull_stop {
            2
        } else {
            prev.map_or(0, |p| p.pending)
        };
        match dest {
            1 => note.tech.flags.insert(TechFlags::HAMMER_ON),
            2 => note.tech.flags.insert(TechFlags::PULL_OFF),
            _ => {}
        }
        own
    };
    *slot = Some(Last { pending });
}

fn voicing_of(f: &Frame, strings: usize) -> Option<Voicing> {
    if strings == 0 {
        return None;
    }
    let mut frets: Vec<Option<u8>> = vec![None; strings];
    let mut fingers: Vec<Option<u8>> = vec![None; strings];
    let mut any_finger = false;
    let mut starts: Vec<(usize, u8)> = Vec::new();
    let mut stops: Vec<(usize, u8)> = Vec::new();
    for n in &f.notes {
        let idx = (n.string as usize).checked_sub(1)?;
        if idx >= strings {
            continue;
        }
        let fret = u8::try_from(n.fret).ok()?;
        frets[idx] = Some(fret);
        if n.finger.is_some() {
            any_finger = true;
            fingers[idx] = n.finger;
        }
        match n.barre {
            Some(true) => starts.push((idx, fret)),
            Some(false) => stops.push((idx, fret)),
            None => {}
        }
    }
    if frets.iter().all(Option::is_none) {
        return None;
    }
    let mut barres = Vec::new();
    for (si, sf) in &starts {
        if let Some((ei, _)) = stops.iter().find(|(_, f)| f == sf) {
            barres.push(Barre {
                fret: *sf,
                from: *si.min(ei),
                to: *si.max(ei),
            });
            break;
        }
    }
    if !any_finger {
        match assign_fingers(&frets) {
            Some((fi, ba)) => {
                fingers = fi;
                if barres.is_empty() {
                    barres = ba;
                }
            }
            None => fingers = vec![None; strings],
        }
    }
    let base_fret = u8::try_from(f.first_fret.clamp(1, 36)).unwrap_or(1);
    Some(Voicing {
        frets,
        fingers,
        barres,
        base_fret,
    })
}
