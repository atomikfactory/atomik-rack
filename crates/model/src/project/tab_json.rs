//! Reading and writing a note clip's optional `"tab"` and a song's optional `"sheets"` (see
//! [`crate::tab_data`] and [`crate::chord_sheets`]).
//!
//! A clip's `"tab"` is an object:
//!
//! ```text
//! "tab": {
//!   "instrument": "guitar6",
//!   "tuning": {"frets": 24, "strings": [[open, start_fret, course, octave_course, "label"], ...]},
//!   "capo": {"fret": 2, "strings": mask},            // only with a capo; "strings" only when partial
//!   "notes": [[tick, key, string, fret, technique, [plain velocity, plain length]], ...],
//!   "diagrams": [[tick, "name", [fret or null, ...], [finger or null, ...], [[fret, from, to]], base_fret], ...],
//!   "lyrics": [[tick, "syllable"], ...]
//! }
//! ```
//!
//! A note's last two elements are optional (`technique` is an object or `null`); the technique
//! object holds `"flags"` (a list of names), `"slide"`, `"vibrato"`, `"harmonic"`
//! (`{"kind", "fret"}`), `"bend"` and `"whammy"` (`[[position, cents], ...]`), `"tremolo"`,
//! `"grace"` (`{"fret", "ticks", "on_beat"}`) and `"strum"`. A song's `"sheets"` is a list of
//! `{"title", "artist", "key", "capo", "meta", "blocks"}` whose blocks are `["section", text]`,
//! `["line", text, [[char index, "chord"], ...], bar]`, `["tab", [lines]]`, `["blank"]` and
//! `["comment", text]`.

use super::write_clip::list_each;
use super::*;
use crate::chord_sheets::{MAX_SHEETS, check_sheets};
use crate::patch::{JsonPath, array, number, object, string, uint};
use crate::tab_data::{
    MAX_TAB_DIAGRAMS, MAX_TAB_LYRICS, MAX_TAB_NOTES, PlainNote, TabData, TabDiagram, TabLyric,
    TabNoteRef, ids,
};
use atomik_tab::chord::ChordSymbol;
use atomik_tab::instrument::{MAX_STRINGS, StringDef};
use atomik_tab::score::{Bend, BendPoint, Grace, Harmonic, Techniques, limits};
use atomik_tab::sheet::{ChordSheet, SheetBlock, TabBlockText};
use atomik_tab::transpose::Key;
use atomik_tab::voicing::{Barre, Voicing};
use atomik_tab::{Capo, Tuning};

fn f32_text(x: f32) -> String {
    if x == 0.0 && x.is_sign_negative() {
        "-0.0".to_owned()
    } else {
        format!("{x}")
    }
}

// ---------------------------------------------------------------------------------------------
// Writing

fn write_bend(w: &mut String, b: &Bend) {
    w.push('[');
    for (i, p) in b.points.iter().enumerate() {
        let sep = if i == 0 { "" } else { ", " };
        let _ = write!(w, "{sep}[{}, {}]", f32_text(p.pos), p.cents);
    }
    w.push(']');
}

/// The technique object of a note (`{}` for no technique at all).
fn write_techniques(w: &mut String, t: &Techniques) {
    w.push('{');
    let mut first = true;
    let mut field = |w: &mut String, name: &str| {
        if !first {
            w.push_str(", ");
        }
        first = false;
        let _ = write!(w, "\"{name}\": ");
    };
    let names: Vec<String> = ids::FLAGS
        .iter()
        .filter(|(f, _)| t.flags.contains(*f))
        .map(|(_, n)| format!("\"{n}\""))
        .collect();
    if !names.is_empty() {
        field(w, "flags");
        let _ = write!(w, "[{}]", names.join(", "));
    }
    if let Some(s) = t.slide {
        field(w, "slide");
        let _ = write!(w, "\"{}\"", ids::slide_id(s));
    }
    if let Some(v) = t.vibrato {
        field(w, "vibrato");
        let _ = write!(w, "\"{}\"", ids::vibrato_id(v));
    }
    if let Some(h) = &t.harmonic {
        field(w, "harmonic");
        let _ = write!(w, "{{\"kind\": \"{}\"", ids::harmonic_id(h.kind));
        if let Some(f) = h.fret {
            let _ = write!(w, ", \"fret\": {}", f32_text(f));
        }
        w.push('}');
    }
    if let Some(b) = &t.bend {
        field(w, "bend");
        write_bend(w, b);
    }
    if let Some(b) = &t.whammy {
        field(w, "whammy");
        write_bend(w, b);
    }
    if t.tremolo_division != 0 {
        field(w, "tremolo");
        let _ = write!(w, "{}", t.tremolo_division);
    }
    if let Some(g) = &t.grace {
        field(w, "grace");
        let _ = write!(
            w,
            "{{\"fret\": {}, \"ticks\": {}, \"on_beat\": {}}}",
            g.fret, g.ticks, g.on_beat
        );
    }
    if let Some(s) = t.strum {
        field(w, "strum");
        let _ = write!(w, "\"{}\"", ids::strum_id(s));
    }
    w.push('}');
}

fn write_opt_list(w: &mut String, list: &[Option<u8>]) {
    w.push('[');
    for (i, x) in list.iter().enumerate() {
        let sep = if i == 0 { "" } else { ", " };
        match x {
            Some(v) => {
                let _ = write!(w, "{sep}{v}");
            }
            None => {
                let _ = write!(w, "{sep}null");
            }
        }
    }
    w.push(']');
}

/// Writes a note clip's `"tab"` after its expression (nothing for a clip without tab data). An
/// entry of a note the clip no longer has is left out.
pub(super) fn write_tab(w: &mut String, c: &Clip, indent: &str) {
    let (Some(tab), ClipContent::Notes(notes)) = (&c.midi.tab, &c.content) else {
        return;
    };
    let kept: Vec<&TabNoteRef> = tab
        .notes
        .iter()
        .filter(|n| {
            notes
                .binary_search_by_key(&(n.tick, n.key), |m| (m.tick, m.key))
                .is_ok()
        })
        .collect();
    let inner = format!("{indent}  ");
    let _ = write!(
        w,
        ",
{indent}\"tab\": {{
{inner}\"instrument\": {},
{inner}\"tuning\": {{\"frets\": {}, \"strings\": ",
        json_str(&tab.instrument_id),
        tab.tuning.frets
    );
    list_each(w, &inner, &tab.tuning.strings, |w, s| {
        let _ = write!(
            w,
            "[{}, {}, {}, {}, {}]",
            s.open,
            s.start_fret,
            s.course,
            s.octave_course,
            json_str(&s.label)
        );
    });
    w.push('}');
    if tab.capo != Capo::NONE {
        let _ = write!(w, ",\n{inner}\"capo\": {{\"fret\": {}", tab.capo.fret);
        if let Some(mask) = tab.capo.strings {
            let _ = write!(w, ", \"strings\": {mask}");
        }
        w.push('}');
    }
    if !kept.is_empty() {
        let _ = write!(w, ",\n{inner}\"notes\": ");
        list_each(w, &inner, &kept, |w, n| {
            let _ = write!(w, "[{}, {}, {}, {}", n.tick, n.key, n.string, n.fret);
            if n.tech.is_empty() && n.plain.is_none() {
                w.push(']');
                return;
            }
            w.push_str(", ");
            if n.tech.is_empty() {
                w.push_str("null");
            } else {
                write_techniques(w, &n.tech);
            }
            if let Some(p) = n.plain {
                let _ = write!(w, ", [{}, {}]", f32_text(p.velocity), p.length);
            }
            w.push(']');
        });
    }
    if !tab.diagrams.is_empty() {
        let _ = write!(w, ",\n{inner}\"diagrams\": ");
        list_each(w, &inner, &tab.diagrams, |w, d| {
            let _ = write!(w, "[{}, {}, ", d.tick, json_str(&d.name));
            write_opt_list(w, &d.voicing.frets);
            w.push_str(", ");
            write_opt_list(w, &d.voicing.fingers);
            w.push_str(", [");
            for (i, b) in d.voicing.barres.iter().enumerate() {
                let sep = if i == 0 { "" } else { ", " };
                let _ = write!(w, "{sep}[{}, {}, {}]", b.fret, b.from, b.to);
            }
            let _ = write!(w, "], {}]", d.voicing.base_fret);
        });
    }
    if !tab.lyrics.is_empty() {
        let _ = write!(w, ",\n{inner}\"lyrics\": ");
        list_each(w, &inner, &tab.lyrics, |w, l| {
            let _ = write!(w, "[{}, {}]", l.tick, json_str(&l.text));
        });
    }
    let _ = write!(w, "\n{indent}}}");
}

fn write_sheet(w: &mut String, s: &ChordSheet) {
    let _ = write!(
        w,
        "{{\"title\": {}, \"artist\": {}",
        json_str(&s.title),
        json_str(&s.artist)
    );
    if let Some(k) = s.key {
        let _ = write!(w, ", \"key\": {}", json_str(&k.name()));
    }
    if s.capo != 0 {
        let _ = write!(w, ", \"capo\": {}", s.capo);
    }
    if !s.meta.is_empty() {
        w.push_str(", \"meta\": [");
        for (i, (k, v)) in s.meta.iter().enumerate() {
            let sep = if i == 0 { "" } else { ", " };
            let _ = write!(w, "{sep}[{}, {}]", json_str(k), json_str(v));
        }
        w.push(']');
    }
    w.push_str(", \"blocks\": [");
    for (i, b) in s.blocks.iter().enumerate() {
        let sep = if i == 0 { "" } else { ", " };
        w.push_str(sep);
        match b {
            SheetBlock::Section(t) => {
                let _ = write!(w, "[\"section\", {}]", json_str(t));
            }
            SheetBlock::Comment(t) => {
                let _ = write!(w, "[\"comment\", {}]", json_str(t));
            }
            SheetBlock::Blank => w.push_str("[\"blank\"]"),
            SheetBlock::Tab(t) => {
                let lines: Vec<String> = t.lines.iter().map(|l| json_str(l)).collect();
                let _ = write!(w, "[\"tab\", [{}]]", lines.join(", "));
            }
            SheetBlock::Line { text, chords, bar } => {
                let cs: Vec<String> = chords
                    .iter()
                    .map(|(at, c)| format!("[{at}, {}]", json_str(&c.name())))
                    .collect();
                let _ = write!(w, "[\"line\", {}, [{}]", json_str(text), cs.join(", "));
                if let Some(bar) = bar {
                    let _ = write!(w, ", {bar}");
                }
                w.push(']');
            }
        }
    }
    w.push_str("]}");
}

/// Writes the song's `"sheets"` (nothing without a sheet) before its tracks.
pub(super) fn write_sheets(w: &mut String, song: &Song) {
    if song.sheets.is_empty() {
        return;
    }
    let _ = write!(w, "    \"sheets\": ");
    list_each(w, "    ", &song.sheets, write_sheet);
    let _ = writeln!(w, ",");
}

// ---------------------------------------------------------------------------------------------
// Reading

fn small(v: &Value, path: &JsonPath, max: u64) -> Result<u8, ModelError> {
    Ok(uint(v, path, max)? as u8)
}

fn read_bend(v: &Value, path: &JsonPath) -> Result<Bend, ModelError> {
    let items = array(v, path)?;
    if items.len() > limits::MAX_CURVE_POINTS {
        return Err(path.error(format!("at most {} points", limits::MAX_CURVE_POINTS)));
    }
    let mut points = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let p = path.index(i);
        let a = tuple(item, &p, 2, "a point [position, cents]")?;
        let pos = number(&a[0], &p.index(0))? as f32;
        let cents = int(&a[1], &p.index(1))?;
        if !pos.is_finite() || !(0.0..=1.0).contains(&pos) {
            return Err(p.index(0).error("a position is 0..=1"));
        }
        let cents = i32::try_from(cents)
            .ok()
            .filter(|c| (-4800..=4800).contains(c))
            .ok_or_else(|| p.index(1).error("cents are -4800..=4800"))?;
        points.push(BendPoint { pos, cents });
    }
    Ok(Bend { points })
}

fn read_techniques(v: &Value, path: &JsonPath) -> Result<Techniques, ModelError> {
    let obj = object(
        v,
        path,
        &[
            "flags", "slide", "vibrato", "harmonic", "bend", "whammy", "tremolo", "grace", "strum",
        ],
        &[],
    )?;
    let mut t = Techniques::default();
    if let Some(f) = obj.get("flags") {
        let fp = path.field("flags");
        for (i, name) in array(f, &fp)?.iter().enumerate() {
            let name = string(name, &fp.index(i))?;
            let flag = ids::FLAGS
                .iter()
                .find(|(_, n)| *n == name)
                .map(|(f, _)| *f)
                .ok_or_else(|| fp.index(i).error(format!("unknown technique {name:?}")))?;
            t.flags.insert(flag);
        }
    }
    if let Some(s) = obj.get("slide") {
        let sp = path.field("slide");
        let name = string(s, &sp)?;
        t.slide = Some(
            ids::slide_from_id(name).ok_or_else(|| sp.error(format!("unknown slide {name:?}")))?,
        );
    }
    if let Some(s) = obj.get("vibrato") {
        let sp = path.field("vibrato");
        let name = string(s, &sp)?;
        t.vibrato = Some(
            ids::vibrato_from_id(name)
                .ok_or_else(|| sp.error(format!("unknown vibrato {name:?}")))?,
        );
    }
    if let Some(h) = obj.get("harmonic") {
        let hp = path.field("harmonic");
        let o = object(h, &hp, &["kind", "fret"], &["kind"])?;
        let kp = hp.field("kind");
        let name = string(&o["kind"], &kp)?;
        let kind = ids::harmonic_from_id(name)
            .ok_or_else(|| kp.error(format!("unknown harmonic {name:?}")))?;
        let fret = match o.get("fret") {
            Some(f) => {
                let x = number(f, &hp.field("fret"))? as f32;
                if !x.is_finite() || !(0.0..=36.0).contains(&x) {
                    return Err(hp.field("fret").error("a harmonic fret is 0..=36"));
                }
                Some(x)
            }
            None => None,
        };
        t.harmonic = Some(Harmonic { kind, fret });
    }
    if let Some(b) = obj.get("bend") {
        t.bend = Some(read_bend(b, &path.field("bend"))?);
    }
    if let Some(b) = obj.get("whammy") {
        t.whammy = Some(read_bend(b, &path.field("whammy"))?);
    }
    if let Some(d) = obj.get("tremolo") {
        let n = uint(d, &path.field("tremolo"), 64)?;
        if !matches!(n, 4 | 8 | 16 | 32 | 64) {
            return Err(path
                .field("tremolo")
                .error("a division is 4, 8, 16, 32 or 64"));
        }
        t.tremolo_division = n as u8;
    }
    if let Some(g) = obj.get("grace") {
        let gp = path.field("grace");
        let o = object(g, &gp, &["fret", "ticks", "on_beat"], &["fret", "ticks"])?;
        t.grace = Some(Grace {
            fret: small(&o["fret"], &gp.field("fret"), 36)?,
            ticks: length(&o["ticks"], &gp.field("ticks"))?,
            on_beat: flag(o, &gp, "on_beat")?,
        });
    }
    if let Some(s) = obj.get("strum") {
        let sp = path.field("strum");
        let name = string(s, &sp)?;
        t.strum = Some(
            ids::strum_from_id(name)
                .ok_or_else(|| sp.error(format!("unknown strum direction {name:?}")))?,
        );
    }
    Ok(t)
}

fn read_opt_list(
    v: &Value,
    path: &JsonPath,
    len: usize,
    max: u64,
) -> Result<Vec<Option<u8>>, ModelError> {
    let items = array(v, path)?;
    if items.len() != len {
        return Err(path.error(format!("expected {len} entries")));
    }
    items
        .iter()
        .enumerate()
        .map(|(i, x)| match x {
            Value::Null => Ok(None),
            _ => Ok(Some(small(x, &path.index(i), max)?)),
        })
        .collect()
}

fn read_tuning(v: &Value, path: &JsonPath) -> Result<Tuning, ModelError> {
    let obj = object(v, path, &["frets", "strings"], &["frets", "strings"])?;
    let frets = small(&obj["frets"], &path.field("frets"), 255)?;
    let sp = path.field("strings");
    let items = array(&obj["strings"], &sp)?;
    if items.len() > MAX_STRINGS {
        return Err(sp.error(format!("at most {MAX_STRINGS} strings")));
    }
    let mut strings = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let p = sp.index(i);
        let a = tuple(
            item,
            &p,
            5,
            "a string [open key, start fret, course, octave course, label]",
        )?;
        strings.push(StringDef {
            open: small(&a[0], &p.index(0), 127)?,
            start_fret: small(&a[1], &p.index(1), 255)?,
            course: small(&a[2], &p.index(2), 255)?,
            octave_course: boolean(&a[3], &p.index(3))?,
            label: string(&a[4], &p.index(4))?.to_owned(),
        });
    }
    Ok(Tuning { strings, frets })
}

fn read_capo(v: &Value, path: &JsonPath) -> Result<Capo, ModelError> {
    let obj = object(v, path, &["fret", "strings"], &["fret"])?;
    let fret = small(&obj["fret"], &path.field("fret"), 255)?;
    let strings = match obj.get("strings") {
        Some(m) => Some(uint(m, &path.field("strings"), u64::from(u16::MAX))? as u16),
        None => None,
    };
    Ok(Capo { fret, strings })
}

/// Reads a note clip's `"tab"` (module docs); the caller checks the whole data.
pub(super) fn load_tab(v: &Value, path: &JsonPath) -> Result<TabData, ModelError> {
    let obj = object(
        v,
        path,
        &[
            "instrument",
            "tuning",
            "capo",
            "notes",
            "diagrams",
            "lyrics",
        ],
        &["instrument", "tuning"],
    )?;
    let instrument = string(&obj["instrument"], &path.field("instrument"))?;
    let tuning = read_tuning(&obj["tuning"], &path.field("tuning"))?;
    let capo = match obj.get("capo") {
        Some(c) => read_capo(c, &path.field("capo"))?,
        None => Capo::NONE,
    };
    let mut tab = TabData::with_tuning(instrument, tuning, capo);
    if let Some(list) = obj.get("notes") {
        let lp = path.field("notes");
        let items = array(list, &lp)?;
        if items.len() > MAX_TAB_NOTES {
            return Err(lp.error(format!("at most {MAX_TAB_NOTES} notes")));
        }
        tab.notes.reserve(items.len());
        for (i, item) in items.iter().enumerate() {
            let p = lp.index(i);
            let a = match item.as_array() {
                Some(a) if (4..=6).contains(&a.len()) => a.as_slice(),
                _ => {
                    return Err(p.error(format!(
                        "expected a note [tick, key, string, fret, technique, plain], found {item}"
                    )));
                }
            };
            let mut n = TabNoteRef::new(
                tick(&a[0], &p.index(0))?,
                small(&a[1], &p.index(1), 127)?,
                small(&a[2], &p.index(2), 255)?,
                small(&a[3], &p.index(3), 255)?,
            );
            if let Some(t) = a.get(4)
                && !t.is_null()
            {
                n.tech = read_techniques(t, &p.index(4))?;
            }
            if let Some(plain) = a.get(5) {
                let pp = p.index(5);
                let pa = tuple(plain, &pp, 2, "[plain velocity, plain length]")?;
                n.plain = Some(PlainNote {
                    velocity: number(&pa[0], &pp.index(0))? as f32,
                    length: length(&pa[1], &pp.index(1))?,
                });
            }
            tab.notes.push(n);
        }
    }
    if let Some(list) = obj.get("diagrams") {
        let lp = path.field("diagrams");
        let items = array(list, &lp)?;
        if items.len() > MAX_TAB_DIAGRAMS {
            return Err(lp.error(format!("at most {MAX_TAB_DIAGRAMS} diagrams")));
        }
        let strings = tab.tuning.len();
        for (i, item) in items.iter().enumerate() {
            let p = lp.index(i);
            let a = tuple(
                item,
                &p,
                6,
                "a diagram [tick, name, frets, fingers, barres, base fret]",
            )?;
            let mut barres = Vec::new();
            let bp = p.index(4);
            let blist = array(&a[4], &bp)?;
            if blist.len() > 1 {
                return Err(bp.error("a voicing has at most one barre"));
            }
            for (j, b) in blist.iter().enumerate() {
                let q = bp.index(j);
                let ba = tuple(b, &q, 3, "a barre [fret, from, to]")?;
                barres.push(Barre {
                    fret: small(&ba[0], &q.index(0), 255)?,
                    from: uint(&ba[1], &q.index(1), 255)? as usize,
                    to: uint(&ba[2], &q.index(2), 255)? as usize,
                });
            }
            tab.diagrams.push(TabDiagram {
                tick: tick(&a[0], &p.index(0))?,
                name: string(&a[1], &p.index(1))?.to_owned(),
                voicing: Voicing {
                    frets: read_opt_list(&a[2], &p.index(2), strings, 255)?,
                    fingers: read_opt_list(&a[3], &p.index(3), strings, 255)?,
                    barres,
                    base_fret: small(&a[5], &p.index(5), 255)?,
                },
            });
        }
    }
    if let Some(list) = obj.get("lyrics") {
        let lp = path.field("lyrics");
        let items = array(list, &lp)?;
        if items.len() > MAX_TAB_LYRICS {
            return Err(lp.error(format!("at most {MAX_TAB_LYRICS} syllables")));
        }
        for (i, item) in items.iter().enumerate() {
            let p = lp.index(i);
            let a = tuple(item, &p, 2, "a syllable [tick, text]")?;
            tab.lyrics.push(TabLyric {
                tick: tick(&a[0], &p.index(0))?,
                text: string(&a[1], &p.index(1))?.to_owned(),
            });
        }
    }
    Ok(tab)
}

fn read_chord(v: &Value, path: &JsonPath) -> Result<ChordSymbol, ModelError> {
    let name = string(v, path)?;
    ChordSymbol::parse(name).map_err(|e| path.error(format!("{name:?} is not a chord: {e}")))
}

fn read_sheet(v: &Value, path: &JsonPath) -> Result<ChordSheet, ModelError> {
    let obj = object(
        v,
        path,
        &["title", "artist", "key", "capo", "meta", "blocks"],
        &["title", "artist", "blocks"],
    )?;
    let mut sheet = ChordSheet {
        title: string(&obj["title"], &path.field("title"))?.to_owned(),
        artist: string(&obj["artist"], &path.field("artist"))?.to_owned(),
        ..ChordSheet::default()
    };
    if let Some(k) = obj.get("key") {
        let kp = path.field("key");
        let name = string(k, &kp)?;
        sheet.key =
            Some(Key::parse(name).ok_or_else(|| kp.error(format!("{name:?} is not a key")))?);
    }
    if let Some(c) = obj.get("capo") {
        sheet.capo = small(c, &path.field("capo"), 255)?;
    }
    if let Some(m) = obj.get("meta") {
        let mp = path.field("meta");
        let items = array(m, &mp)?;
        if items.len() > crate::chord_sheets::MAX_SHEET_META {
            return Err(mp.error("too many facts"));
        }
        for (i, item) in items.iter().enumerate() {
            let p = mp.index(i);
            let a = tuple(item, &p, 2, "a fact [name, value]")?;
            sheet.meta.push((
                string(&a[0], &p.index(0))?.to_owned(),
                string(&a[1], &p.index(1))?.to_owned(),
            ));
        }
    }
    let bp = path.field("blocks");
    let items = array(&obj["blocks"], &bp)?;
    if items.len() > atomik_tab::sheet::MAX_LINES {
        return Err(bp.error("too many lines"));
    }
    for (i, item) in items.iter().enumerate() {
        let p = bp.index(i);
        let a = array(item, &p)?;
        let kind = match a.first() {
            Some(k) => string(k, &p.index(0))?,
            None => return Err(p.error("a block starts with its kind")),
        };
        let expect = |n: usize| -> Result<(), ModelError> {
            if a.len() == n {
                Ok(())
            } else {
                Err(p.error(format!("a {kind} block has {n} elements")))
            }
        };
        sheet.blocks.push(match kind {
            "section" => {
                expect(2)?;
                SheetBlock::Section(string(&a[1], &p.index(1))?.to_owned())
            }
            "comment" => {
                expect(2)?;
                SheetBlock::Comment(string(&a[1], &p.index(1))?.to_owned())
            }
            "blank" => {
                expect(1)?;
                SheetBlock::Blank
            }
            "tab" => {
                expect(2)?;
                let lp = p.index(1);
                let lines = array(&a[1], &lp)?;
                if lines.len() > crate::chord_sheets::MAX_TAB_BLOCK_LINES {
                    return Err(lp.error("too many lines"));
                }
                SheetBlock::Tab(TabBlockText {
                    lines: lines
                        .iter()
                        .enumerate()
                        .map(|(j, l)| string(l, &lp.index(j)).map(str::to_owned))
                        .collect::<Result<_, _>>()?,
                })
            }
            "line" => {
                if !(3..=4).contains(&a.len()) {
                    return Err(p.error("a line block is [\"line\", text, chords, bar]"));
                }
                let cp = p.index(2);
                let list = array(&a[2], &cp)?;
                if list.len() > atomik_tab::sheet::MAX_CHORDS_PER_LINE {
                    return Err(cp.error("too many chords on a line"));
                }
                let mut chords = Vec::with_capacity(list.len());
                for (j, c) in list.iter().enumerate() {
                    let q = cp.index(j);
                    let ca = tuple(c, &q, 2, "a chord [character index, name]")?;
                    chords.push((
                        uint(&ca[0], &q.index(0), 1 << 20)? as usize,
                        read_chord(&ca[1], &q.index(1))?,
                    ));
                }
                let bar = match a.get(3) {
                    Some(Value::Null) | None => None,
                    Some(b) => Some(uint(b, &p.index(3), u64::from(u32::MAX))? as u32),
                };
                SheetBlock::Line {
                    text: string(&a[1], &p.index(1))?.to_owned(),
                    chords,
                    bar,
                }
            }
            other => return Err(p.index(0).error(format!("unknown block kind {other:?}"))),
        });
    }
    Ok(sheet)
}

/// Reads the song's optional `"sheets"`.
pub(super) fn load_sheets(
    v: Option<&Value>,
    path: &JsonPath,
) -> Result<Vec<ChordSheet>, ModelError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let items = array(v, path)?;
    if items.len() > MAX_SHEETS {
        return Err(path.error(format!("at most {MAX_SHEETS} sheets")));
    }
    let sheets: Vec<ChordSheet> = items
        .iter()
        .enumerate()
        .map(|(i, item)| read_sheet(item, &path.index(i)))
        .collect::<Result<_, _>>()?;
    check_sheets(&sheets).map_err(|m| path.error(m))?;
    Ok(sheets)
}
