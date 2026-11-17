//! GPIF, the XML score of `.gpx` (Guitar Pro 6) and `.gp` (Guitar Pro 7 and 8) files.
//!
//! The document is a graph held together by ids: the master bars list a bar per staff, a bar lists
//! its voices, a voice its beats, a beat its rhythm and its notes. Tracks carry the tuning and
//! capo as properties. Decisions this reader makes (the format has no public description; they come
//! from the author's understanding and are checked only against the test fixtures):
//!
//! * String numbers count from the **lowest** string (0); the `Tuning` property lists the pitches in
//!   the same order. Tab order is the reverse.
//! * Bend and whammy values are in 1/50 of a semitone, like the binary formats, with offsets in
//!   percent of the note.
//! * A tempo value is `bpm unit` where the unit is 1 eighth, 2 quarter, 3 dotted quarter, 4 half,
//!   5 dotted half; the tempo is converted to quarter notes per minute.
//! * Tremolo picking `1/4`, `1/2`, `3/4` mean 8th, 16th and 32nd strokes.

use std::collections::BTreeMap;

use super::dom::{
    self, Bar, Beat, Diagram, Doc, Info, Lyrics, MAX_VOICES, MasterBar, Note, Track, WHOLE,
    bend_units_to_cents,
};
use super::xml::{El, Xml};
use super::{GpError, Warnings, limits};
use crate::score::{
    Bend, BendPoint, Grace, Harmonic, HarmonicKind, KeySignature, PPQ, SlideKind, StrumDirection,
    TechFlags, Techniques, VibratoKind,
};

/// Reads the XML of a GPIF score.
pub(super) fn read(bytes: &[u8], w: &mut Warnings) -> Result<Doc, GpError> {
    let xml = Xml::parse(bytes)?;
    let root = xml.root();
    if root.name() != "GPIF" {
        return Err(GpError::Corrupt("the XML is not a Guitar Pro score".into()));
    }
    let mut cx = Cx::new(root, w);
    cx.doc()
}

fn table<'a>(
    parent: Option<El<'a>>,
    group: &str,
    item: &str,
    key: &str,
) -> BTreeMap<&'a str, El<'a>> {
    let mut m = BTreeMap::new();
    if let Some(g) = parent.and_then(|p| p.child(group)) {
        for e in g.all(item) {
            if let Some(id) = e.attr(key) {
                m.insert(id, e);
            }
        }
    }
    m
}

/// A property by name: the `Property` element under `Properties`.
fn prop<'a>(owner: El<'a>, name: &str) -> Option<El<'a>> {
    owner
        .child("Properties")?
        .all("Property")
        .find(|p| p.attr("name") == Some(name))
}

fn prop_enabled(owner: El<'_>, name: &str) -> bool {
    prop(owner, name).is_some_and(|p| match p.child_text("Enable") {
        // `<Enable/>` is on; some writers spell the switch out.
        Some(t) => !t.eq_ignore_ascii_case("false") && t != "0",
        None => matches!(p.text().trim(), "true" | "True" | "1"),
    })
}

fn num<T: std::str::FromStr>(s: Option<&str>) -> Option<T> {
    s.and_then(|s| s.trim().parse().ok())
}

fn float(owner: El<'_>, name: &str) -> Option<f64> {
    let p = prop(owner, name)?;
    num(p.child_text("Float"))
        .or_else(|| num(p.child_text("Number")))
        .or_else(|| num(Some(p.text())))
}

fn ids(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace()
}

struct Dur {
    units: i64,
}

struct Cx<'a, 'w> {
    root: El<'a>,
    w: &'w mut Warnings,
    rhythms: BTreeMap<&'a str, Dur>,
    beats: BTreeMap<&'a str, El<'a>>,
    notes: BTreeMap<&'a str, El<'a>>,
    voices: BTreeMap<&'a str, El<'a>>,
    bars: BTreeMap<&'a str, El<'a>>,
    beats_seen: usize,
    notes_seen: usize,
}

/// What a track needs while its bars are read.
struct TrackCx<'a> {
    strings: usize,
    percussion: bool,
    /// `[element][articulation]` MIDI numbers of a drum kit.
    drum_keys: Vec<Vec<u8>>,
    diagrams: BTreeMap<&'a str, Diagram>,
    velocity: u8,
}

impl<'a, 'w> Cx<'a, 'w> {
    fn new(root: El<'a>, w: &'w mut Warnings) -> Self {
        let mut rhythms = BTreeMap::new();
        if let Some(g) = root.child("Rhythms") {
            for r in g.all("Rhythm") {
                if let Some(id) = r.attr("id") {
                    rhythms.insert(id, rhythm(r));
                }
            }
        }
        Self {
            root,
            w,
            rhythms,
            beats: table(Some(root), "Beats", "Beat", "id"),
            notes: table(Some(root), "Notes", "Note", "id"),
            voices: table(Some(root), "Voices", "Voice", "id"),
            bars: table(Some(root), "Bars", "Bar", "id"),
            beats_seen: 0,
            notes_seen: 0,
        }
    }

    fn doc(&mut self) -> Result<Doc, GpError> {
        let root = self.root;
        let mut doc = Doc {
            info: info(root.child("Score")),
            tempo: 120.0,
            ..Doc::default()
        };

        // Tracks in master order.
        let track_els: BTreeMap<&str, El<'_>> = table(Some(root), "Tracks", "Track", "id");
        let doc_order: Vec<El<'_>> = root
            .child("Tracks")
            .map(|t| t.all("Track").collect())
            .unwrap_or_default();
        let mut ordered: Vec<El<'_>> = Vec::new();
        if let Some(list) = root
            .child("MasterTrack")
            .and_then(|m| m.child_text("Tracks"))
        {
            for id in ids(list) {
                if let Some(t) = track_els.get(id) {
                    ordered.push(*t);
                }
            }
        }
        if ordered.len() != doc_order.len() {
            ordered = doc_order;
        }
        if ordered.len() > limits::MAX_TRACKS {
            return Err(GpError::TooMany("tracks"));
        }

        let masters: Vec<El<'_>> = root
            .child("MasterBars")
            .map(|m| m.all("MasterBar").collect())
            .unwrap_or_default();
        if masters.len() > limits::MAX_BARS {
            return Err(GpError::TooMany("bars"));
        }
        if masters.is_empty() || ordered.is_empty() {
            return Err(GpError::Corrupt(
                "the score has no bars or no tracks".into(),
            ));
        }
        doc.master_bars = masters.iter().map(|m| master_bar(*m, self.w)).collect();
        self.tempo(&mut doc);

        // Which entries of a master bar's `Bars` list belong to which track.
        let mut first_staff = Vec::with_capacity(ordered.len());
        let mut at = 0usize;
        for t in &ordered {
            first_staff.push(at);
            at += t
                .child("Staves")
                .map_or(1, |s| s.all("Staff").count().max(1));
        }

        let mut lyrics = None;
        for (ti, t) in ordered.iter().enumerate() {
            let (mut track, mut tcx) = self.track_head(*t);
            for m in &masters {
                let bar_id = m
                    .child_text("Bars")
                    .and_then(|b| ids(b).nth(first_staff[ti]))
                    .and_then(|id| self.bars.get(id).copied());
                let bar = match bar_id {
                    Some(b) => self.bar(b, &mut tcx)?,
                    None => Bar::default(),
                };
                track.bars.push(bar);
            }
            if lyrics.is_none()
                && let Some(l) = track_lyrics(*t, ti)
            {
                lyrics = Some(l);
            }
            doc.tracks.push(track);
        }
        doc.lyrics = lyrics;
        Ok(doc)
    }

    /// Tempo automations go to the master bar they sit in.
    fn tempo(&mut self, doc: &mut Doc) {
        let Some(autos) = self
            .root
            .child("MasterTrack")
            .and_then(|m| m.child("Automations"))
        else {
            return;
        };
        for a in autos.all("Automation") {
            if !a
                .child_text("Type")
                .is_some_and(|t| t.eq_ignore_ascii_case("tempo"))
            {
                continue;
            }
            let bar = num::<usize>(a.child_text("Bar")).unwrap_or(0);
            let pos = num::<f64>(a.child_text("Position")).unwrap_or(0.0);
            let value = a.child_text("Value").unwrap_or("");
            let mut parts = value.split_whitespace();
            let Some(bpm) = parts.next().and_then(|v| v.parse::<f64>().ok()) else {
                continue;
            };
            let unit = parts
                .next()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(2);
            let factor = match unit {
                1 => 0.5,
                3 => 1.5,
                4 => 2.0,
                5 => 3.0,
                _ => 1.0,
            };
            let q = bpm * factor;
            if !q.is_finite() || q <= 0.0 {
                continue;
            }
            if let Some(mb) = doc.master_bars.get_mut(bar) {
                mb.tempo_auto.push((
                    if pos.is_finite() {
                        pos.clamp(0.0, 1.0)
                    } else {
                        0.0
                    },
                    q,
                ));
            }
        }
    }

    fn track_head(&mut self, t: El<'a>) -> (Track, TrackCx<'a>) {
        // Properties live on the staff (Guitar Pro 7) or on the track itself (Guitar Pro 6).
        let staff = t.child("Staves").and_then(|s| s.child("Staff"));
        let find = |name: &str| staff.and_then(|s| prop(s, name)).or_else(|| prop(t, name));

        let pitches: Vec<u8> = find("Tuning")
            .and_then(|p| p.child_text("Pitches"))
            .map(|s| {
                ids(s)
                    .filter_map(|v| v.parse::<i32>().ok())
                    .map(|v| v.clamp(0, 127) as u8)
                    .collect()
            })
            .unwrap_or_default();
        let instrument_set_type = t
            .child("InstrumentSet")
            .and_then(|i| i.child_text("Type"))
            .unwrap_or("");
        let percussion = instrument_set_type.eq_ignore_ascii_case("drumKit")
            || t.child("GeneralMidi")
                .is_some_and(|g| g.attr("table") == Some("Percussion"));
        let mut opens: Vec<u8> = pitches.into_iter().take(12).collect();
        opens.reverse();
        if opens.is_empty() {
            if !percussion {
                self.w
                    .warn("a track has no tuning; standard guitar tuning was used");
            }
            opens = vec![64, 59, 55, 50, 45, 40];
        }
        let capo = find("CapoFret")
            .and_then(|p| num::<u32>(p.child_text("Fret")))
            .unwrap_or(0);
        let frets = find("FretCount")
            .and_then(|p| num::<u32>(p.child_text("Fret")))
            .unwrap_or(24);
        let program = t
            .child("GeneralMidi")
            .and_then(|g| num::<u32>(g.child_text("Program")))
            .or_else(|| {
                t.child("Sounds")
                    .and_then(|s| s.child("Sound"))
                    .and_then(|s| s.child("MIDI"))
                    .and_then(|m| num::<u32>(m.child_text("Program")))
            })
            .unwrap_or(25);
        let name = t.child_text("Name").unwrap_or("").to_owned();
        let instrument_ref = t
            .child("Instrument")
            .and_then(|i| i.attr("ref"))
            .unwrap_or("");
        let strings = opens.len();

        let mut drum_keys = Vec::new();
        if let Some(els) = t.child("InstrumentSet").and_then(|i| i.child("Elements")) {
            for e in els.all("Element").take(256) {
                let keys: Vec<u8> = e
                    .child("Articulations")
                    .map(|a| {
                        a.all("Articulation")
                            .take(16)
                            .map(|x| {
                                num::<u32>(x.child_text("OutputMidiNumber"))
                                    .unwrap_or(38)
                                    .min(127) as u8
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                drum_keys.push(keys);
            }
        }

        let mut diagrams = BTreeMap::new();
        if let Some(items) = find("DiagramCollection").and_then(|p| p.child("Items")) {
            for it in items.all("Item").take(512) {
                if let Some(id) = it.attr("id") {
                    diagrams.insert(id, diagram(it, strings));
                }
            }
        }

        let track = Track {
            name,
            opens,
            frets: u8::try_from(frets.clamp(1, 99)).unwrap_or(24),
            capo: u8::try_from(capo.min(99)).unwrap_or(0),
            program: u8::try_from(program.min(127)).unwrap_or(25),
            percussion,
            banjo: instrument_ref.to_ascii_lowercase().contains("banjo"),
            twelve_string: instrument_ref.to_ascii_lowercase().contains("12"),
            bars: Vec::new(),
        };
        let tcx = TrackCx {
            strings,
            percussion,
            drum_keys,
            diagrams,
            velocity: 95,
        };
        (track, tcx)
    }

    fn bar(&mut self, bar: El<'a>, tcx: &mut TrackCx<'a>) -> Result<Bar, GpError> {
        let mut out = Bar::default();
        let Some(list) = bar.child_text("Voices") else {
            return Ok(out);
        };
        for id in ids(list).take(MAX_VOICES) {
            if id.starts_with('-') {
                continue;
            }
            let Some(v) = self.voices.get(id).copied() else {
                continue;
            };
            let beats = self.voice(v, tcx)?;
            if !beats.is_empty() {
                out.voices.push(beats);
            }
        }
        Ok(out)
    }

    fn voice(&mut self, voice: El<'a>, tcx: &mut TrackCx<'a>) -> Result<Vec<Beat>, GpError> {
        let mut out: Vec<Beat> = Vec::new();
        let mut graces: Vec<(u8, Grace)> = Vec::new();
        let Some(list) = voice.child_text("Beats") else {
            return Ok(out);
        };
        for id in ids(list) {
            self.beats_seen += 1;
            if self.beats_seen > limits::MAX_BEATS {
                return Err(GpError::TooMany("beats"));
            }
            if out.len() >= limits::MAX_BEATS_PER_BAR {
                return Err(GpError::Corrupt("a bar has too many beats".into()));
            }
            let Some(b) = self.beats.get(id).copied() else {
                continue;
            };
            let units = b
                .child("Rhythm")
                .and_then(|r| r.attr("ref"))
                .and_then(|r| self.rhythms.get(r))
                .map_or(WHOLE / 4, |d| d.units);
            let (mut beat, fx) = self.beat(b, units, tcx)?;
            if let Some(kind) = b.child_text("GraceNotes") {
                // A grace beat takes no time: it decorates the next beat.
                let on_beat = kind.eq_ignore_ascii_case("OnBeat");
                for n in &beat.notes {
                    graces.push((
                        n.string,
                        Grace {
                            fret: n.fret.min(60),
                            ticks: ((units + dom::K / 2) / dom::K).clamp(1, PPQ),
                            on_beat,
                        },
                    ));
                }
                continue;
            }
            for n in &mut beat.notes {
                if let Some((_, g)) = graces.iter().rev().find(|(s, _)| *s == n.string) {
                    n.tech.grace = Some(*g);
                }
                apply_fx(n, &fx);
            }
            graces.clear();
            out.push(beat);
        }
        dom::place(&mut out);
        Ok(out)
    }

    fn beat(
        &mut self,
        b: El<'a>,
        units: i64,
        tcx: &mut TrackCx<'a>,
    ) -> Result<(Beat, BeatFx), GpError> {
        if let Some(d) = b.child_text("Dynamic") {
            tcx.velocity = dynamic_velocity(d);
        }
        let mut beat = Beat {
            units,
            ..Beat::default()
        };
        let mut fx = BeatFx::default();
        for name in ["Brush", "Arpeggio"] {
            if let Some(p) = prop(b, name) {
                fx.strum = direction(p.child_text("Direction"));
            }
            if let Some(d) = b.child_text(name) {
                fx.strum = direction(Some(d));
            }
        }
        if fx.strum.is_none()
            && let Some(p) = prop(b, "PickStroke")
        {
            fx.strum = direction(p.child_text("Direction"));
        }
        fx.slap = prop_enabled(b, "Slapped");
        fx.pop = prop_enabled(b, "Popped");
        fx.fade_in = b
            .child_text("Fadding")
            .is_some_and(|f| f.eq_ignore_ascii_case("FadeIn"));
        fx.tremolo = match b.child_text("Tremolo") {
            Some("1/4") => 8,
            Some("1/2") => 16,
            Some("3/4") => 32,
            _ => 0,
        };
        fx.whammy = curve(b, "WhammyBar");
        if let Some(c) = b.child_text("Chord")
            && let Some(d) = tcx.diagrams.get(c.trim())
        {
            beat.diagram = Some(d.clone());
        }

        if let Some(list) = b.child_text("Notes") {
            for id in ids(list) {
                self.notes_seen += 1;
                if self.notes_seen > limits::MAX_NOTES {
                    return Err(GpError::TooMany("notes"));
                }
                let Some(n) = self.notes.get(id).copied() else {
                    continue;
                };
                if let Some(note) = self.note(n, tcx) {
                    beat.notes.push(note);
                }
            }
        }
        Ok((beat, fx))
    }

    fn note(&mut self, n: El<'a>, tcx: &TrackCx<'a>) -> Option<Note> {
        let mut tech = Techniques::default();
        let string_prop = prop(n, "String").and_then(|p| num::<usize>(p.child_text("String")));
        let fret_prop = prop(n, "Fret").and_then(|p| num::<i32>(p.child_text("Fret")));
        let (string, fret) = if tcx.percussion {
            let key = self.drum_key(n, tcx);
            (0u8, key)
        } else {
            let s = string_prop.unwrap_or(0);
            if s >= tcx.strings {
                self.w
                    .warn("a note on a string the track does not have was dropped");
                return None;
            }
            let fret = fret_prop.filter(|f| (0..=99).contains(f)).unwrap_or(0);
            (
                u8::try_from(tcx.strings - 1 - s).unwrap_or(0),
                u8::try_from(fret).unwrap_or(0),
            )
        };

        if let Some(a) = num::<u32>(n.child_text("Accent")) {
            if a & 1 != 0 {
                tech.flags.insert(TechFlags::STACCATO);
            }
            if a & 4 != 0 {
                tech.flags.insert(TechFlags::ACCENT);
            }
            if a & 8 != 0 {
                tech.flags.insert(TechFlags::HEAVY_ACCENT);
            }
        }
        if n.has("AntiAccent") {
            tech.flags.insert(TechFlags::GHOST);
        }
        if n.has("LetRing") {
            tech.flags.insert(TechFlags::LET_RING);
        }
        match n.child_text("Vibrato") {
            Some("Slight") => tech.vibrato = Some(VibratoKind::Normal),
            Some("Wide") => tech.vibrato = Some(VibratoKind::Wide),
            Some(_) => tech.vibrato = Some(VibratoKind::Normal),
            None => {}
        }
        if prop_enabled(n, "PalmMuted") {
            tech.flags.insert(TechFlags::PALM_MUTE);
        }
        if prop_enabled(n, "Muted") {
            tech.flags.insert(TechFlags::DEAD);
        }
        if prop_enabled(n, "Tapped") {
            tech.flags.insert(TechFlags::TAP);
        }
        if prop_enabled(n, "Slapped") {
            tech.flags.insert(TechFlags::SLAP);
        }
        if prop_enabled(n, "Popped") {
            tech.flags.insert(TechFlags::POP);
        }
        if let Some(p) = prop(n, "Slide") {
            let flags = num::<u32>(p.child_text("Flags")).unwrap_or(0);
            const ORDER: [(u32, SlideKind); 6] = [
                (1, SlideKind::Shift),
                (2, SlideKind::Legato),
                (4, SlideKind::OutDown),
                (8, SlideKind::OutUp),
                (16, SlideKind::InFromBelow),
                (32, SlideKind::InFromAbove),
            ];
            tech.slide = ORDER.iter().find(|(m, _)| flags & m != 0).map(|(_, k)| *k);
        }
        if let Some(p) = prop(n, "HarmonicType") {
            let fret = prop(n, "HarmonicFret").and_then(|f| num::<f32>(f.child_text("HFret")));
            let kind = match p.child_text("HType").unwrap_or("") {
                "Natural" => Some(HarmonicKind::Natural),
                "Artificial" => Some(HarmonicKind::Artificial),
                "Pinch" => Some(HarmonicKind::Pinch),
                "Tap" => Some(HarmonicKind::Tapped),
                "Semi" => Some(HarmonicKind::Semi),
                _ => {
                    self.w.warn("an unknown kind of harmonic was ignored");
                    None
                }
            };
            tech.harmonic = kind.map(|k| Harmonic {
                kind: k,
                fret: if matches!(k, HarmonicKind::Artificial | HarmonicKind::Tapped) {
                    fret
                } else {
                    None
                },
            });
        }
        if prop_enabled(n, "Bended") {
            tech.bend = curve(n, "Bend");
        }
        if prop(n, "Trill").is_some() || n.has("Trill") {
            self.w.warn("trills are not imported");
        }
        let tie = n
            .child("Tie")
            .is_some_and(|t| t.attr("destination") == Some("true"));
        Some(Note {
            string,
            fret,
            velocity: tcx.velocity,
            tie,
            hopo_origin: prop_enabled(n, "HopoOrigin"),
            hopo_dest: prop_enabled(n, "HopoDestination"),
            tech,
        })
    }

    /// The MIDI key of a drum note from its element and variation.
    fn drum_key(&mut self, n: El<'a>, tcx: &TrackCx<'a>) -> u8 {
        let index = |name: &str| -> usize {
            prop(n, name)
                .and_then(|p| p.children().find_map(|c| num::<f64>(Some(c.text()))))
                .map_or(0, |v| v.max(0.0) as usize)
        };
        let (e, v) = (index("Element"), index("Variation"));
        tcx.drum_keys
            .get(e)
            .and_then(|k| k.get(v).or_else(|| k.first()))
            .copied()
            .unwrap_or(38)
    }
}

/// The effects of a beat that every note takes.
#[derive(Default)]
struct BeatFx {
    strum: Option<StrumDirection>,
    slap: bool,
    pop: bool,
    fade_in: bool,
    tremolo: u8,
    whammy: Option<Bend>,
}

fn apply_fx(n: &mut Note, fx: &BeatFx) {
    let t = &mut n.tech;
    if fx.slap {
        t.flags.insert(TechFlags::SLAP);
    }
    if fx.pop {
        t.flags.insert(TechFlags::POP);
    }
    if fx.fade_in {
        t.flags.insert(TechFlags::FADE_IN);
    }
    if t.strum.is_none() {
        t.strum = fx.strum;
    }
    if t.tremolo_division == 0 {
        t.tremolo_division = fx.tremolo;
    }
    if t.whammy.is_none() {
        t.whammy = fx.whammy.clone();
    }
}

fn direction(d: Option<&str>) -> Option<StrumDirection> {
    match d? {
        "Up" => Some(StrumDirection::Up),
        "Down" => Some(StrumDirection::Down),
        _ => None,
    }
}

fn dynamic_velocity(d: &str) -> u8 {
    match d.trim() {
        "PPP" => 15,
        "PP" => 31,
        "P" => 47,
        "MP" => 63,
        "MF" => 79,
        "FF" => 111,
        "FFF" => 127,
        _ => 95,
    }
}

/// A bend or whammy curve from `<prefix>OriginValue`, `...MiddleValue`, `...DestinationValue` and
/// the offsets (percent of the note).
fn curve(owner: El<'_>, prefix: &str) -> Option<Bend> {
    let get = |s: &str| float(owner, &format!("{prefix}{s}"));
    let origin = get("OriginValue");
    let middle = get("MiddleValue");
    let dest = get("DestinationValue");
    if origin.is_none() && middle.is_none() && dest.is_none() {
        return None;
    }
    let off = |s: &str, default: f64| (get(s).unwrap_or(default) / 100.0).clamp(0.0, 1.0) as f32;
    let cents = |v: f64| bend_units_to_cents(v.round() as i64);
    let mut b = Bend::default();
    b.points.push(BendPoint {
        pos: off("OriginOffset", 0.0),
        cents: cents(origin.unwrap_or(0.0)),
    });
    if let Some(m) = middle {
        b.points.push(BendPoint {
            pos: off("MiddleOffset1", 33.0),
            cents: cents(m),
        });
        b.points.push(BendPoint {
            pos: off("MiddleOffset2", 66.0),
            cents: cents(m),
        });
    }
    b.points.push(BendPoint {
        pos: off("DestinationOffset", 100.0),
        cents: cents(dest.unwrap_or(0.0)),
    });
    b.sanitize();
    Some(b)
}

fn rhythm(r: El<'_>) -> Dur {
    let den: i64 = match r.child_text("NoteValue").unwrap_or("Quarter") {
        "Whole" => 1,
        "Half" => 2,
        "Eighth" => 8,
        "16th" => 16,
        "32nd" => 32,
        "64th" => 64,
        "128th" => 128,
        "256th" => 256,
        _ => 4,
    };
    let mut units = WHOLE / den;
    if let Some(d) = r.child("AugmentationDot") {
        let n = num::<u32>(d.attr("count")).unwrap_or(1).min(3);
        // 1 dot: x3/2, 2 dots: x7/4, 3 dots: x15/8.
        let pow = 1i64 << n;
        units = units * (2 * pow - 1) / pow;
    }
    if let Some(t) = r.child("PrimaryTuplet") {
        let n = num::<i64>(t.attr("num")).unwrap_or(1).clamp(1, 64);
        let d = num::<i64>(t.attr("den")).unwrap_or(1).clamp(1, 64);
        units = units * d / n;
    }
    Dur {
        units: units.max(1),
    }
}

fn info(score: Option<El<'_>>) -> Info {
    let Some(s) = score else {
        return Info::default();
    };
    let t = |n: &str| s.child_text(n).unwrap_or("").to_owned();
    let mut i = Info {
        title: t("Title"),
        subtitle: t("SubTitle"),
        artist: t("Artist"),
        album: t("Album"),
        words: t("Words"),
        music: t("Music"),
        tab_by: t("Tabber"),
        copyright: t("Copyright"),
        notices: t("Notices"),
    };
    let both = t("WordsAndMusic");
    if i.words.is_empty() {
        i.words = both.clone();
    }
    if i.music.is_empty() {
        i.music = both;
    }
    i
}

fn master_bar(m: El<'_>, w: &mut Warnings) -> MasterBar {
    let (num_s, den_s) = m
        .child_text("Time")
        .and_then(|t| t.split_once('/'))
        .unwrap_or(("4", "4"));
    let num_v = num::<u8>(Some(num_s)).filter(|n| *n > 0).unwrap_or(4);
    let den_v = num::<u8>(Some(den_s))
        .filter(|d| d.is_power_of_two() && *d <= 64)
        .unwrap_or(4);
    let mut mb = MasterBar::new(num_v, den_v);
    if let Some(r) = m.child("Repeat") {
        mb.repeat_open = r.attr("start") == Some("true");
        if r.attr("end") == Some("true") {
            let count = num::<u32>(r.attr("count")).unwrap_or(2).clamp(2, 255);
            mb.repeat_close = Some(u8::try_from(count).unwrap_or(2));
        }
    }
    if let Some(a) = m.child_text("AlternateEndings") {
        let mut mask = 0u8;
        for n in ids(a).filter_map(|v| v.parse::<u32>().ok()) {
            if (1..=8).contains(&n) {
                mask |= 1 << (n - 1);
            }
        }
        mb.alt = mask;
    }
    if let Some(s) = m.child("Section") {
        let text = s.child_text("Text").unwrap_or("");
        let letter = s.child_text("Letter").unwrap_or("");
        let name = if text.is_empty() { letter } else { text };
        if !name.is_empty() {
            mb.marker = Some(name.to_owned());
        }
    }
    if let Some(k) = m.child("Key") {
        let fifths = num::<i32>(k.child_text("AccidentalCount"))
            .unwrap_or(0)
            .clamp(-7, 7);
        let minor = k
            .child_text("Mode")
            .is_some_and(|m| m.eq_ignore_ascii_case("minor"));
        mb.key = Some(KeySignature {
            fifths: i8::try_from(fifths).unwrap_or(0),
            minor,
        });
    }
    if m.has("Directions") || m.has("Jump") {
        w.warn("jump directions (D.C., D.S., coda) are not followed");
    }
    mb
}

fn diagram(item: El<'_>, strings: usize) -> Diagram {
    let name = item.attr("name").unwrap_or("").to_owned();
    let mut frets: Vec<Option<u8>> = vec![None; strings];
    if let Some(d) = item.child("Diagram") {
        let base = num::<i32>(d.attr("baseFret")).unwrap_or(0);
        let entries: Vec<(usize, i32)> = d
            .all("Fret")
            .filter_map(|f| Some((num::<usize>(f.attr("string"))?, num::<i32>(f.attr("fret"))?)))
            .collect();
        // Frets below the window start are relative to it.
        let relative = base > 1 && entries.iter().any(|(_, f)| *f > 0 && *f < base);
        for (s, f) in entries {
            if s < strings && f >= 0 {
                let f = if relative && f > 0 { f + base - 1 } else { f };
                frets[strings - 1 - s] = u8::try_from(f.min(40)).ok();
            }
        }
    }
    Diagram { name, frets }
}

fn track_lyrics(t: El<'_>, index: usize) -> Option<Lyrics> {
    let l = t.child("Lyrics")?;
    let mut out = Lyrics {
        track: index,
        lines: Vec::new(),
    };
    for line in l.all("Line").take(5) {
        let text = line.child_text("Text").unwrap_or("");
        if text.trim().is_empty() {
            continue;
        }
        let offset = num::<usize>(line.child_text("Offset")).unwrap_or(0);
        out.lines.push((offset, text.to_owned()));
    }
    (!out.lines.is_empty()).then_some(out)
}
