//! The MusicXML writer: a [`TabScore`] as a partwise document.
//!
//! Every track is a part with a tab staff (`staff-details` with the tuning and the capo). The score
//! is cut into measures by its meter map; notes that cross a bar line are split into tied
//! pieces, overlapping notes go to further voices, durations that are not a plain or dotted note
//! value become tuplets (or tied pieces), and `divisions` is the least common multiple that
//! represents every position exactly (capped, beyond the cap positions are rounded and a warning is
//! given).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::{MusicXmlError, limits};
use crate::chord::ChordSymbol;
use crate::gp::{ImportWarning, Warnings};
use crate::score::{
    BendKind, HarmonicKind, KeySignature, PPQ, SlideKind, StrumDirection, TabNote, TabScore,
    TabTrack, TechFlags, VibratoKind, bar_ticks, limits as score_limits,
};
use crate::voicing::Voicing;

const VOICES: usize = 4;

/// The plain note values, longest first: `(type, ticks)`.
const PLAIN: [(&str, i64); 10] = [
    ("breve", 8 * PPQ),
    ("whole", 4 * PPQ),
    ("half", 2 * PPQ),
    ("quarter", PPQ),
    ("eighth", PPQ / 2),
    ("16th", PPQ / 4),
    ("32nd", PPQ / 8),
    ("64th", PPQ / 16),
    ("128th", PPQ / 32),
    ("256th", PPQ / 64),
];

/// Tuplet ratios `(actual, normal)` tried in this order.
const TUPLETS: [(i64, i64); 12] = [
    (3, 2),
    (5, 4),
    (6, 4),
    (7, 4),
    (9, 8),
    (10, 8),
    (11, 8),
    (12, 8),
    (13, 8),
    (15, 8),
    (5, 2),
    (3, 1),
];

const SHARP_NAMES: [(&str, i8); 12] = [
    ("C", 0),
    ("C", 1),
    ("D", 0),
    ("D", 1),
    ("E", 0),
    ("F", 0),
    ("F", 1),
    ("G", 0),
    ("G", 1),
    ("A", 0),
    ("A", 1),
    ("B", 0),
];

const LETTERS: [char; 7] = ['C', 'D', 'E', 'F', 'G', 'A', 'B'];

type Grid = [(i64, i64, (u8, u8))];

#[derive(Clone, Copy, Debug)]
struct Notated {
    ty: &'static str,
    dots: u8,
    tm: Option<(i64, i64)>,
}

fn dotted(base: i64, dots: u8) -> i64 {
    base * ((2 << dots) - 1) / (1 << dots)
}

fn notate(ticks: i64) -> Option<Notated> {
    for (ty, base) in PLAIN {
        for dots in 0..=2u8 {
            if dotted(base, dots) == ticks {
                return Some(Notated { ty, dots, tm: None });
            }
        }
    }
    for (actual, normal) in TUPLETS {
        for (ty, base) in PLAIN {
            if base * normal % actual == 0 && base * normal / actual == ticks {
                return Some(Notated {
                    ty,
                    dots: 0,
                    tm: Some((actual, normal)),
                });
            }
        }
    }
    None
}

/// Splits a length that has no single notation into plain pieces, longest first.
fn split_len(len: i64) -> Vec<(i64, Notated)> {
    if let Some(n) = notate(len) {
        return vec![(len, n)];
    }
    let mut out = Vec::new();
    let mut left = len;
    while left > 0 && out.len() < 64 {
        let mut best: Option<(i64, Notated)> = None;
        for (ty, base) in PLAIN {
            for dots in (0..=2u8).rev() {
                let v = dotted(base, dots);
                if v <= left && best.is_none_or(|b| v > b.0) {
                    best = Some((v, Notated { ty, dots, tm: None }));
                }
            }
        }
        let (v, n) = best.unwrap_or((
            left,
            // Shorter than the shortest note value.
            Notated {
                ty: "256th",
                dots: 0,
                tm: None,
            },
        ));
        out.push((v, n));
        left -= v;
    }
    out
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn pitch_xml(key: u8) -> String {
    let (step, alter) = SHARP_NAMES[usize::from(key % 12)];
    let octave = i32::from(key / 12) - 1;
    if alter == 0 {
        format!("<pitch><step>{step}</step><octave>{octave}</octave></pitch>")
    } else {
        format!("<pitch><step>{step}</step><alter>{alter}</alter><octave>{octave}</octave></pitch>")
    }
}

/// One note of an item.
#[derive(Clone)]
struct OutNote {
    /// Index into the track's notes.
    note: usize,
    /// The first piece of the note: it carries the techniques, the grace note and the lyric.
    first: bool,
    tie_stop: bool,
    tie_start: bool,
}

#[derive(Clone)]
struct Item {
    start: i64,
    end: i64,
    notated: Notated,
    /// The notes; empty for a rest or a gap.
    notes: Vec<OutNote>,
    /// A gap in a voice after the first (written as `forward`).
    gap: bool,
    tuplet_start: bool,
    tuplet_stop: bool,
}

/// Links from a note to its neighbours on the string.
#[derive(Default, Clone, Copy)]
struct Marks {
    hammer_start: bool,
    pull_start: bool,
    /// A slide of this kind ends here.
    slide_stop: Option<SlideKind>,
    /// The next note on the string continues this one (a tie starts).
    tie_start: bool,
}

struct Piece {
    note: usize,
    start: i64,
    end: i64,
    first: bool,
    last: bool,
}

struct BarOut {
    start: i64,
    meter: (u8, u8),
    voices: Vec<Vec<Item>>,
}

struct TrackOut {
    bars: Vec<BarOut>,
    marks: Vec<Marks>,
}

pub(super) fn write(score: &TabScore) -> Result<(String, Vec<ImportWarning>), MusicXmlError> {
    if score.tracks.is_empty() {
        return Err(MusicXmlError::NoMusic);
    }
    if score.tracks.len() > score_limits::MAX_TRACKS {
        return Err(MusicXmlError::TooMany("tracks"));
    }
    let mut w = Warnings::default();
    let grid = bar_grid(score);
    let outs: Vec<TrackOut> = score
        .tracks
        .iter()
        .map(|t| layout_track(t, &grid, &mut w))
        .collect();
    let div = choose_divisions(score, &outs, &grid, &mut w);

    let mut x = String::with_capacity(64 * 1024);
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    x.push_str("<score-partwise version=\"4.0\">\n");
    write_header(&mut x, score);
    write_part_list(&mut x, score);
    for (ti, (t, out)) in score.tracks.iter().zip(&outs).enumerate() {
        write_part(&mut x, score, ti, t, out, &grid, div, &mut w);
    }
    x.push_str("</score-partwise>\n");
    Ok((x, w.finish()))
}

// ---------------------------------------------------------------- grid and layout

/// `(start, len, meter)` of every bar, covering all the music, chords, lyrics and maps.
fn bar_grid(score: &TabScore) -> Vec<(i64, i64, (u8, u8))> {
    let mut end = 0i64;
    for t in &score.tracks {
        end = end.max(t.end_tick());
        for (tick, _) in &t.lyrics {
            end = end.max(tick + 1);
        }
        for (tick, _) in &t.chords {
            end = end.max(tick + 1);
        }
        for d in &t.diagrams {
            end = end.max(d.tick + 1);
        }
    }
    for (tick, _) in &score.tempo {
        end = end.max(tick + 1);
    }
    for (tick, _) in &score.markers {
        end = end.max(tick + 1);
    }
    let mut meters = score.meter.clone();
    meters.sort_by_key(|m| m.0);
    let mut out = Vec::new();
    let (mut num, mut den) = (4u8, 4u8);
    let (mut tick, mut bar, mut mi) = (0i64, 0u32, 0usize);
    while (tick < end || out.is_empty()) && out.len() < score_limits::MAX_BARS {
        while mi < meters.len() && meters[mi].0 <= bar {
            num = meters[mi].1.max(1);
            den = meters[mi].2.max(1);
            mi += 1;
        }
        let len = bar_ticks(num, den);
        out.push((tick, len, (num, den)));
        tick += len;
        bar += 1;
    }
    out
}

fn bar_of(grid: &Grid, tick: i64) -> usize {
    match grid.binary_search_by(|g| g.0.cmp(&tick)) {
        Ok(i) => i,
        Err(0) => 0,
        Err(i) => i - 1,
    }
}

fn layout_track(t: &TabTrack, grid: &Grid, w: &mut Warnings) -> TrackOut {
    let n = t.notes.len();
    let mut marks = vec![Marks::default(); n];
    if !t.is_percussion {
        let mut by_string: BTreeMap<u8, Vec<usize>> = BTreeMap::new();
        for (i, note) in t.notes.iter().enumerate() {
            by_string.entry(note.string).or_default().push(i);
        }
        for list in by_string.values_mut() {
            list.sort_by_key(|&i| (t.notes[i].tick, i));
            for k in 1..list.len() {
                let (p, i) = (list[k - 1], list[k]);
                let cur = &t.notes[i];
                if t.notes[p].tick >= cur.tick {
                    continue;
                }
                if cur.tie {
                    marks[p].tie_start = true;
                    continue;
                }
                if cur.tech.has(TechFlags::HAMMER_ON) {
                    marks[p].hammer_start = true;
                }
                if cur.tech.has(TechFlags::PULL_OFF) {
                    marks[p].pull_start = true;
                }
                if let Some(s @ (SlideKind::Legato | SlideKind::Shift)) = t.notes[p].tech.slide {
                    marks[i].slide_stop = Some(s);
                }
            }
        }
    }
    // Pieces per bar.
    let mut per_bar: Vec<Vec<Piece>> = (0..grid.len()).map(|_| Vec::new()).collect();
    for (i, note) in t.notes.iter().enumerate() {
        let mut tick = note.tick.max(0);
        let end = tick.saturating_add(note.len.max(1));
        let mut first = true;
        while tick < end {
            let b = bar_of(grid, tick);
            let (bs, bl, _) = grid[b];
            if tick >= bs + bl {
                break;
            }
            let piece_end = end.min(bs + bl);
            per_bar[b].push(Piece {
                note: i,
                start: tick - bs,
                end: piece_end - bs,
                first,
                last: piece_end == end,
            });
            first = false;
            tick = piece_end;
        }
    }
    let mut bars = Vec::with_capacity(grid.len());
    for (b, &(start, len, meter)) in grid.iter().enumerate() {
        let voices = layout_bar(t, &marks, &per_bar[b], len, w);
        bars.push(BarOut {
            start,
            meter,
            voices,
        });
    }
    TrackOut { bars, marks }
}

/// Groups the pieces of a bar into chords, places the chords in voices and fills the gaps.
fn layout_bar(
    t: &TabTrack,
    marks: &[Marks],
    pieces: &[Piece],
    len: i64,
    w: &mut Warnings,
) -> Vec<Vec<Item>> {
    // Chords: pieces with the same start and end.
    let mut groups: BTreeMap<(i64, i64), Vec<&Piece>> = BTreeMap::new();
    for p in pieces {
        groups.entry((p.start, p.end)).or_default().push(p);
    }
    let mut order: Vec<((i64, i64), Vec<&Piece>)> = groups.into_iter().collect();
    // Same start: the longer chord first (it takes the lower voice).
    order.sort_by_key(|((s, e), _)| (*s, std::cmp::Reverse(*e)));
    let mut cursors = [0i64; VOICES];
    let mut voices: Vec<Vec<Item>> = vec![Vec::new()];
    for ((s, e), mut ps) in order {
        let Some(v) = (0..VOICES).find(|&v| cursors[v] <= s) else {
            w.warn("notes that overlap more than four voices deep were dropped");
            continue;
        };
        while voices.len() <= v {
            voices.push(Vec::new());
        }
        if s > cursors[v] {
            push_gap(&mut voices[v], cursors[v], s, v > 0);
        }
        ps.sort_by_key(|p| t.notes[p.note].string);
        let parts = split_len(e - s);
        let count = parts.len();
        let mut at = s;
        for (k, (plen, notated)) in parts.into_iter().enumerate() {
            let last_part = k + 1 == count;
            let notes = ps
                .iter()
                .map(|p| {
                    let first = p.first && k == 0;
                    let last = p.last && last_part;
                    OutNote {
                        note: p.note,
                        first,
                        tie_stop: !first || (t.notes[p.note].tie && p.first),
                        tie_start: !last || (last && marks[p.note].tie_start),
                    }
                })
                .collect();
            voices[v].push(Item {
                start: at,
                end: at + plen,
                notated,
                notes,
                gap: false,
                tuplet_start: false,
                tuplet_stop: false,
            });
            at += plen;
        }
        cursors[v] = e;
    }
    // The first voice runs to the end of the bar.
    if cursors[0] < len {
        push_gap(&mut voices[0], cursors[0], len, false);
    }
    for v in &mut voices {
        mark_tuplets(v);
    }
    voices
}

fn push_gap(voice: &mut Vec<Item>, from: i64, to: i64, forward: bool) {
    let mut at = from;
    for (plen, notated) in split_len(to - from) {
        voice.push(Item {
            start: at,
            end: at + plen,
            notated,
            notes: Vec::new(),
            gap: forward,
            tuplet_start: false,
            tuplet_stop: false,
        });
        at += plen;
    }
}

/// Marks where tuplet brackets start and stop along a voice.
fn mark_tuplets(items: &mut [Item]) {
    // The open bracket: ratio, the base value of its first item, ticks played so far.
    let mut open: Option<((i64, i64), i64, i64)> = None;
    let mut last_tm: Option<usize> = None;
    for i in 0..items.len() {
        let tm = items[i].notated.tm;
        let ticks = items[i].end - items[i].start;
        match (tm, open) {
            (Some(r), Some((cur, base, acc))) if r == cur => {
                let acc = acc + ticks;
                if acc >= base * r.1 {
                    items[i].tuplet_stop = true;
                    open = None;
                } else {
                    open = Some((cur, base, acc));
                }
                last_tm = Some(i);
            }
            (Some(r), prev) => {
                if prev.is_some()
                    && let Some(l) = last_tm
                {
                    items[l].tuplet_stop = true;
                }
                let base = PLAIN
                    .iter()
                    .find(|(ty, _)| *ty == items[i].notated.ty)
                    .map_or(PPQ, |p| p.1);
                items[i].tuplet_start = true;
                if ticks >= base * r.1 {
                    items[i].tuplet_stop = true;
                    open = None;
                } else {
                    open = Some((r, base, ticks));
                }
                last_tm = Some(i);
            }
            (None, Some(_)) => {
                if let Some(l) = last_tm {
                    items[l].tuplet_stop = true;
                }
                open = None;
            }
            (None, None) => {}
        }
    }
    if open.is_some()
        && let Some(l) = last_tm
    {
        items[l].tuplet_stop = true;
    }
}

fn need(v: i64, d: &mut i64, exact: &mut bool) {
    if v <= 0 {
        return;
    }
    let want = PPQ / gcd(v, PPQ);
    let l = *d / gcd(*d, want) * want;
    if l > limits::MAX_WRITE_DIVISIONS {
        *exact = false;
    } else {
        *d = l;
    }
}

/// The `divisions` value: the least common multiple that makes every position exact, capped.
fn choose_divisions(score: &TabScore, outs: &[TrackOut], grid: &Grid, w: &mut Warnings) -> i64 {
    let mut d = 1i64;
    let mut exact = true;
    for g in grid {
        need(g.1, &mut d, &mut exact);
    }
    for o in outs {
        for b in &o.bars {
            for v in &b.voices {
                for it in v {
                    need(it.start, &mut d, &mut exact);
                    need(it.end - it.start, &mut d, &mut exact);
                }
            }
        }
    }
    let rel = |tick: i64| tick - grid[bar_of(grid, tick)].0;
    for (tick, _) in &score.tempo {
        need(rel(*tick), &mut d, &mut exact);
    }
    for (tick, _) in &score.markers {
        need(rel(*tick), &mut d, &mut exact);
    }
    for t in &score.tracks {
        for (tick, _) in &t.chords {
            need(rel(*tick), &mut d, &mut exact);
        }
        for dg in &t.diagrams {
            need(rel(dg.tick), &mut d, &mut exact);
        }
    }
    if !exact {
        w.warn("some positions do not fit a MusicXML time grid exactly and were rounded");
    }
    d.clamp(1, limits::MAX_WRITE_DIVISIONS)
}

// ---------------------------------------------------------------- header and part list

fn write_header(x: &mut String, s: &TabScore) {
    if !s.title.is_empty() {
        let _ = writeln!(
            x,
            "  <work><work-title>{}</work-title></work>",
            esc(&s.title)
        );
    }
    if !s.subtitle.is_empty() {
        let _ = writeln!(x, "  <movement-title>{}</movement-title>", esc(&s.subtitle));
    }
    x.push_str("  <identification>\n");
    for (ty, v) in [
        ("composer", &s.music_by),
        ("lyricist", &s.words_by),
        ("arranger", &s.tab_by),
        ("artist", &s.artist),
    ] {
        if !v.is_empty() {
            let _ = writeln!(x, "    <creator type=\"{ty}\">{}</creator>", esc(v));
        }
    }
    if !s.copyright.is_empty() {
        let _ = writeln!(x, "    <rights>{}</rights>", esc(&s.copyright));
    }
    x.push_str("    <encoding><software>Atomik Rack</software></encoding>\n");
    x.push_str("    <miscellaneous>\n");
    if !s.album.is_empty() {
        let _ = writeln!(
            x,
            "      <miscellaneous-field name=\"atomik-album\">{}</miscellaneous-field>",
            esc(&s.album)
        );
    }
    if !s.notices.is_empty() {
        let _ = writeln!(
            x,
            "      <miscellaneous-field name=\"atomik-notices\">{}</miscellaneous-field>",
            esc(&s.notices)
        );
    }
    for (i, t) in s.tracks.iter().enumerate() {
        if t.is_percussion {
            continue;
        }
        let _ = writeln!(
            x,
            "      <miscellaneous-field name=\"atomik-tuning-P{}\">{}</miscellaneous-field>",
            i + 1,
            esc(&private_tuning(t))
        );
    }
    x.push_str("    </miscellaneous>\n  </identification>\n");
}

fn clean_label(s: &str) -> String {
    s.chars()
        .map(|c| {
            if matches!(c, ';' | '|' | ',' | '=') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

fn private_tuning(t: &TabTrack) -> String {
    let opens: Vec<String> = t
        .tuning
        .strings
        .iter()
        .map(|s| s.open.to_string())
        .collect();
    let strings: Vec<String> = t
        .tuning
        .strings
        .iter()
        .map(|s| {
            format!(
                "{},{},{},{}",
                s.start_fret,
                s.course,
                u8::from(s.octave_course),
                clean_label(&s.label)
            )
        })
        .collect();
    format!(
        "id={};frets={};o={};s={}",
        clean_label(&t.instrument_id),
        t.tuning.frets,
        opens.join(","),
        strings.join("|")
    )
}

/// The `(string, key)` pairs of a percussion track, in the order the instruments are listed.
fn percussion_instruments(t: &TabTrack) -> Vec<(u8, u8)> {
    let mut v: Vec<(u8, u8)> = t.notes.iter().map(|n| (n.string, n.fret)).collect();
    v.sort_unstable();
    v.dedup();
    v.truncate(256);
    v
}

fn write_part_list(x: &mut String, s: &TabScore) {
    x.push_str("  <part-list>\n");
    for (i, t) in s.tracks.iter().enumerate() {
        let id = format!("P{}", i + 1);
        let name = if t.name.is_empty() { "Track" } else { &t.name };
        let _ = writeln!(x, "    <score-part id=\"{id}\">");
        let _ = writeln!(x, "      <part-name>{}</part-name>", esc(name));
        if t.is_percussion {
            let inst = percussion_instruments(t);
            for (k, (_, key)) in inst.iter().enumerate() {
                let _ = writeln!(
                    x,
                    "      <score-instrument id=\"{id}-I{}\"><instrument-name>Percussion {key}</instrument-name></score-instrument>",
                    k + 1
                );
            }
            for (k, (_, key)) in inst.iter().enumerate() {
                let _ = writeln!(
                    x,
                    "      <midi-instrument id=\"{id}-I{}\"><midi-channel>10</midi-channel><midi-unpitched>{}</midi-unpitched></midi-instrument>",
                    k + 1,
                    u32::from(*key) + 1
                );
            }
            if inst.is_empty() {
                let _ = writeln!(
                    x,
                    "      <score-instrument id=\"{id}-I1\"><instrument-name>Percussion</instrument-name></score-instrument>"
                );
            }
        } else {
            let _ = writeln!(
                x,
                "      <score-instrument id=\"{id}-I1\"><instrument-name>{}</instrument-name></score-instrument>",
                esc(name)
            );
            // Channel 10 is for percussion; spread the others over the remaining ones.
            let channel = [1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16][i % 15];
            let _ = writeln!(
                x,
                "      <midi-instrument id=\"{id}-I1\"><midi-channel>{channel}</midi-channel><midi-program>{}</midi-program></midi-instrument>",
                u32::from(t.midi_program.min(127)) + 1
            );
        }
        x.push_str("    </score-part>\n");
    }
    x.push_str("  </part-list>\n");
}

// ---------------------------------------------------------------- parts

fn q(rel: i64, div: i64) -> i64 {
    let v = (i128::from(rel) * i128::from(div) + i128::from(PPQ) / 2).div_euclid(i128::from(PPQ));
    v.clamp(-(1 << 50), 1 << 50) as i64
}

fn dur(start: i64, end: i64, div: i64) -> i64 {
    (q(end, div) - q(start, div)).max(1)
}

fn key_at(score: &TabScore, tick: i64) -> Option<KeySignature> {
    score
        .key
        .iter()
        .filter(|(t, _)| *t <= tick)
        .max_by_key(|(t, _)| *t)
        .map(|(_, k)| *k)
}

/// A direction or harmony placed at a tick of a bar.
struct Event {
    rel: i64,
    xml: String,
}

impl Event {
    /// The element with an `offset` of `off` divisions from the item it is written before.
    fn xml_at(&self, off: i64) -> String {
        if off == 0 {
            return self.xml.clone();
        }
        let tag = format!("<offset>{off}</offset>");
        let mut xml = self.xml.clone();
        // `offset` follows the content of a harmony, and the direction types of a direction
        // (before its `sound`).
        let at = xml
            .find("<sound")
            .or_else(|| xml.rfind("</harmony>"))
            .or_else(|| xml.rfind("</direction>"));
        if let Some(p) = at {
            xml.insert_str(p, &tag);
        }
        xml
    }
}

fn tempo_xml(bpm: f64) -> String {
    let b = if (bpm - bpm.round()).abs() < 1e-9 {
        format!("{}", bpm.round() as i64)
    } else {
        format!("{bpm:.3}")
    };
    format!(
        "<direction placement=\"above\"><direction-type><metronome><beat-unit>quarter</beat-unit><per-minute>{b}</per-minute></metronome></direction-type><sound tempo=\"{b}\"/></direction>"
    )
}

fn kind_value(suffix: &str) -> &'static str {
    match suffix {
        "" => "major",
        "m" => "minor",
        "7" => "dominant",
        "maj7" => "major-seventh",
        "m7" => "minor-seventh",
        "dim" => "diminished",
        "aug" => "augmented",
        "dim7" => "diminished-seventh",
        "m7b5" => "half-diminished",
        "6" => "major-sixth",
        "m6" => "minor-sixth",
        "9" => "dominant-ninth",
        "sus2" => "suspended-second",
        "sus4" => "suspended-fourth",
        "5" => "power",
        _ => "other",
    }
}

fn frame_xml(v: &Voicing, strings: usize) -> String {
    let top = v.frets.iter().flatten().copied().max().unwrap_or(0);
    let base = v.base_fret.max(1);
    let shown = (top.saturating_sub(base) + 1).max(4);
    let mut s = format!(
        "<frame><frame-strings>{strings}</frame-strings><frame-frets>{shown}</frame-frets>"
    );
    if base > 1 {
        let _ = write!(s, "<first-fret>{base}</first-fret>");
    }
    for (i, f) in v.frets.iter().enumerate() {
        let Some(f) = f else {
            continue;
        };
        let _ = write!(s, "<frame-note><string>{}</string><fret>{f}</fret>", i + 1);
        if let Some(Some(finger)) = v.fingers.get(i) {
            let _ = write!(s, "<fingering>{finger}</fingering>");
        }
        for b in &v.barres {
            let ends_ok = v.frets.get(b.from) == Some(&Some(b.fret))
                && v.frets.get(b.to) == Some(&Some(b.fret));
            if ends_ok && b.from == i {
                s.push_str("<barre type=\"start\"/>");
            }
            if ends_ok && b.to == i && b.from != b.to {
                s.push_str("<barre type=\"stop\"/>");
            }
        }
        s.push_str("</frame-note>");
    }
    s.push_str("</frame>");
    s
}

fn harmony_xml(
    chord: Option<&ChordSymbol>,
    name: &str,
    voicing: Option<(&Voicing, usize)>,
) -> String {
    let mut s = String::from("<harmony>");
    match chord {
        Some(c) => {
            let _ = write!(
                s,
                "<root><root-step>{}</root-step>",
                LETTERS[usize::from(c.root.letter % 7)]
            );
            if c.root.accidental != 0 {
                let _ = write!(s, "<root-alter>{}</root-alter>", c.root.accidental);
            }
            s.push_str("</root>");
            let suffix = c.suffix();
            let _ = write!(
                s,
                "<kind text=\"{}\">{}</kind>",
                esc(&suffix),
                kind_value(&suffix)
            );
            if let Some(b) = c.bass {
                let _ = write!(
                    s,
                    "<bass><bass-step>{}</bass-step>",
                    LETTERS[usize::from(b.letter % 7)]
                );
                if b.accidental != 0 {
                    let _ = write!(s, "<bass-alter>{}</bass-alter>", b.accidental);
                }
                s.push_str("</bass>");
            }
        }
        None => {
            let _ = write!(
                s,
                "<root><root-step>C</root-step></root><kind text=\"{}\">none</kind>",
                esc(name)
            );
        }
    }
    if let Some((v, n)) = voicing {
        s.push_str(&frame_xml(v, n));
    }
    s.push_str("</harmony>");
    s
}

fn attributes_xml(
    score: &TabScore,
    t: &TabTrack,
    b: usize,
    bar: &BarOut,
    div: i64,
    written_meter: &mut Option<(u8, u8)>,
    written_key: &mut Option<KeySignature>,
) -> String {
    let mut attrs = String::new();
    if b == 0 {
        let _ = write!(attrs, "<divisions>{div}</divisions>");
    }
    let key = key_at(score, bar.start);
    if let Some(k) = key
        && key != *written_key
    {
        let _ = write!(
            attrs,
            "<key><fifths>{}</fifths><mode>{}</mode></key>",
            k.fifths,
            if k.minor { "minor" } else { "major" }
        );
        *written_key = key;
    }
    if *written_meter != Some(bar.meter) {
        let _ = write!(
            attrs,
            "<time><beats>{}</beats><beat-type>{}</beat-type></time>",
            bar.meter.0, bar.meter.1
        );
        *written_meter = Some(bar.meter);
    }
    if b == 0 {
        if t.is_percussion {
            attrs.push_str("<clef><sign>percussion</sign><line>2</line></clef>");
        } else {
            let n = t.tuning.len();
            attrs.push_str("<clef><sign>TAB</sign><line>5</line></clef>");
            let _ = write!(attrs, "<staff-details><staff-lines>{n}</staff-lines>");
            for (i, s) in t.tuning.strings.iter().enumerate() {
                // Line 1 is the bottom line: the last string in tab order.
                let (step, alter) = SHARP_NAMES[usize::from(s.open % 12)];
                let _ = write!(
                    attrs,
                    "<staff-tuning line=\"{}\"><tuning-step>{step}</tuning-step>",
                    n - i
                );
                if alter != 0 {
                    let _ = write!(attrs, "<tuning-alter>{alter}</tuning-alter>");
                }
                let _ = write!(
                    attrs,
                    "<tuning-octave>{}</tuning-octave></staff-tuning>",
                    i32::from(s.open / 12) - 1
                );
            }
            if t.capo.fret > 0 {
                let _ = write!(attrs, "<capo>{}</capo>", t.capo.fret);
            }
            attrs.push_str("</staff-details>");
        }
    }
    attrs
}

#[allow(clippy::too_many_arguments)]
fn write_part(
    x: &mut String,
    score: &TabScore,
    ti: usize,
    t: &TabTrack,
    out: &TrackOut,
    grid: &Grid,
    div: i64,
    w: &mut Warnings,
) {
    let id = format!("P{}", ti + 1);
    let _ = writeln!(x, "  <part id=\"{id}\">");
    let n_strings = t.tuning.len();
    let inst = if t.is_percussion {
        percussion_instruments(t)
    } else {
        Vec::new()
    };

    // Lyrics by tick, with the hyphen state resolved.
    let mut lyrics: BTreeMap<i64, (String, &'static str)> = BTreeMap::new();
    let mut prev_hyphen = false;
    for (tick, text) in &t.lyrics {
        let hy = text.ends_with('-');
        let stripped = text.trim_end_matches('-');
        if stripped.is_empty() {
            continue;
        }
        let syl = match (prev_hyphen, hy) {
            (false, false) => "single",
            (false, true) => "begin",
            (true, true) => "middle",
            (true, false) => "end",
        };
        prev_hyphen = hy;
        lyrics
            .entry(*tick)
            .or_insert_with(|| (stripped.to_owned(), syl));
    }

    // Directions and harmonies per bar.
    let mut events: Vec<Vec<Event>> = (0..grid.len()).map(|_| Vec::new()).collect();
    if ti == 0 {
        for (tick, bpm) in &score.tempo {
            let b = bar_of(grid, *tick);
            events[b].push(Event {
                rel: tick - grid[b].0,
                xml: tempo_xml(*bpm),
            });
        }
        for (tick, name) in &score.markers {
            let b = bar_of(grid, *tick);
            events[b].push(Event {
                rel: tick - grid[b].0,
                xml: format!(
                    "<direction placement=\"above\"><direction-type><rehearsal>{}</rehearsal></direction-type></direction>",
                    esc(name)
                ),
            });
        }
    }
    let mut covered: Vec<usize> = Vec::new();
    for d in &t.diagrams {
        let b = bar_of(grid, d.tick);
        let chord = ChordSymbol::parse(&d.name).ok();
        if let Some(c) = &chord {
            for (k, (ct, cs)) in t.chords.iter().enumerate() {
                if *ct == d.tick && cs == c {
                    covered.push(k);
                }
            }
        }
        events[b].push(Event {
            rel: d.tick - grid[b].0,
            xml: harmony_xml(chord.as_ref(), &d.name, Some((&d.voicing, n_strings))),
        });
    }
    for (k, (tick, c)) in t.chords.iter().enumerate() {
        if covered.contains(&k) {
            continue;
        }
        let b = bar_of(grid, *tick);
        events[b].push(Event {
            rel: tick - grid[b].0,
            xml: harmony_xml(Some(c), &c.name(), None),
        });
    }
    for e in &mut events {
        e.sort_by_key(|e| e.rel);
    }

    let mut written_meter: Option<(u8, u8)> = None;
    let mut written_key: Option<KeySignature> = None;
    let last_bar = out.bars.len().saturating_sub(1);
    for (b, bar) in out.bars.iter().enumerate() {
        let _ = writeln!(x, "    <measure number=\"{}\">", b + 1);
        let attrs = attributes_xml(score, t, b, bar, div, &mut written_meter, &mut written_key);
        if !attrs.is_empty() {
            let _ = writeln!(x, "      <attributes>{attrs}</attributes>");
        }
        let mut ev_iter = events[b].iter().peekable();
        let nvoices = bar.voices.len().max(1);
        let others_empty = bar.voices.iter().skip(1).all(Vec::is_empty);
        for (vi, voice) in bar.voices.iter().enumerate() {
            let voice_no = vi + 1;
            let whole_rest =
                vi == 0 && others_empty && voice.len() == 1 && voice[0].notes.is_empty();
            for (k, it) in voice.iter().enumerate() {
                if vi == 0 {
                    // Directions and harmonies that start inside this item.
                    while let Some(e) = ev_iter.peek() {
                        if e.rel >= it.end && k + 1 != voice.len() {
                            break;
                        }
                        let off = q(e.rel, div) - q(it.start, div);
                        let _ = writeln!(x, "      {}", e.xml_at(off));
                        ev_iter.next();
                    }
                }
                let d = dur(it.start, it.end, div);
                if !it.notes.is_empty() {
                    write_item(
                        x,
                        t,
                        &id,
                        &inst,
                        it,
                        d,
                        voice_no,
                        &out.marks,
                        &mut lyrics,
                        bar.start,
                        w,
                    );
                } else if it.gap {
                    let _ = writeln!(x, "      <forward><duration>{d}</duration></forward>");
                } else if whole_rest {
                    let _ = writeln!(
                        x,
                        "      <note><rest measure=\"yes\"/><duration>{d}</duration><voice>{voice_no}</voice></note>"
                    );
                } else {
                    let _ = writeln!(
                        x,
                        "      <note><rest/><duration>{d}</duration><voice>{voice_no}</voice>{}</note>",
                        rest_tail(it)
                    );
                }
            }
            if vi + 1 < nvoices {
                let back = q(voice.last().map_or(0, |i| i.end), div);
                if back > 0 {
                    let _ = writeln!(x, "      <backup><duration>{back}</duration></backup>");
                }
            }
        }
        if b == last_bar {
            x.push_str(
                "      <barline location=\"right\"><bar-style>light-heavy</bar-style></barline>\n",
            );
        }
        x.push_str("    </measure>\n");
    }
    if !lyrics.is_empty() {
        w.warn("lyrics that do not sit on a note were dropped");
    }
    x.push_str("  </part>\n");
}

fn type_xml(n: &Notated) -> String {
    let mut s = format!("<type>{}</type>", n.ty);
    for _ in 0..n.dots {
        s.push_str("<dot/>");
    }
    if let Some((a, nn)) = n.tm {
        let _ = write!(
            s,
            "<time-modification><actual-notes>{a}</actual-notes><normal-notes>{nn}</normal-notes></time-modification>"
        );
    }
    s
}

fn tuplet_xml(it: &Item) -> String {
    let mut s = String::new();
    if it.tuplet_start {
        s.push_str("<tuplet type=\"start\"/>");
    }
    if it.tuplet_stop {
        s.push_str("<tuplet type=\"stop\"/>");
    }
    s
}

fn rest_tail(it: &Item) -> String {
    let mut s = type_xml(&it.notated);
    let t = tuplet_xml(it);
    if !t.is_empty() {
        let _ = write!(s, "<notations>{t}</notations>");
    }
    s
}

fn fmt_alter(cents: i32) -> String {
    let semis = f64::from(cents) / 100.0;
    if (semis - semis.round()).abs() < 1e-9 {
        format!("{}", semis.round() as i64)
    } else {
        let mut s = format!("{semis:.3}");
        while s.ends_with('0') {
            s.pop();
        }
        s
    }
}

fn grace_xml(t: &TabTrack, note: &TabNote, voice_no: usize) -> Option<String> {
    let g = note.tech.grace.as_ref()?;
    if t.is_percussion {
        return None;
    }
    let key = t.tuning.pitch(usize::from(note.string), g.fret, t.capo)?;
    let ty = PLAIN
        .iter()
        .skip(3)
        .min_by_key(|(_, ticks)| (ticks - g.ticks).abs())
        .map_or("32nd", |p| p.0);
    let attr = if g.on_beat {
        "steal-time-following=\"50\""
    } else {
        "slash=\"yes\""
    };
    Some(format!(
        "      <note><grace {attr}/>{}<voice>{voice_no}</voice><type>{ty}</type><notations><technical><string>{}</string><fret>{}</fret></technical></notations></note>\n",
        pitch_xml(key),
        usize::from(note.string) + 1,
        g.fret
    ))
}

/// The pieces of the notations of a note: the technical, articulations and ornaments contents.
#[derive(Default)]
struct Notations {
    direct: String,
    tech: String,
    art: String,
    orn: String,
}

fn techniques_xml(note: &TabNote, m: Marks, number: usize, w: &mut Warnings) -> Notations {
    let mut n = Notations::default();
    let tc = &note.tech;
    let mut other: Vec<String> = Vec::new();
    if tc.has(TechFlags::HAMMER_ON) && !note.tie {
        let _ = write!(
            n.tech,
            "<hammer-on type=\"stop\" number=\"{number}\">H</hammer-on>"
        );
    }
    if m.hammer_start {
        let _ = write!(
            n.tech,
            "<hammer-on type=\"start\" number=\"{number}\">H</hammer-on>"
        );
    }
    if tc.has(TechFlags::PULL_OFF) && !note.tie {
        let _ = write!(
            n.tech,
            "<pull-off type=\"stop\" number=\"{number}\">P</pull-off>"
        );
    }
    if m.pull_start {
        let _ = write!(
            n.tech,
            "<pull-off type=\"start\" number=\"{number}\">P</pull-off>"
        );
    }
    match tc.slide {
        Some(SlideKind::Legato) => {
            let _ = write!(
                n.direct,
                "<slide type=\"start\" number=\"{number}\" line-type=\"solid\"/>"
            );
        }
        Some(SlideKind::Shift) => {
            let _ = write!(
                n.direct,
                "<glissando type=\"start\" number=\"{number}\" line-type=\"solid\"/>"
            );
        }
        Some(SlideKind::InFromBelow) => n.art.push_str("<scoop/>"),
        Some(SlideKind::InFromAbove) => n.art.push_str("<plop/>"),
        Some(SlideKind::OutUp) => n.art.push_str("<doit/>"),
        Some(SlideKind::OutDown) => n.art.push_str("<falloff/>"),
        None => {}
    }
    match m.slide_stop {
        Some(SlideKind::Legato) => {
            let _ = write!(n.direct, "<slide type=\"stop\" number=\"{number}\"/>");
        }
        Some(SlideKind::Shift) => {
            let _ = write!(n.direct, "<glissando type=\"stop\" number=\"{number}\"/>");
        }
        _ => {}
    }
    if let Some(b) = &tc.bend {
        let peak = fmt_alter(b.peak());
        match b.kind() {
            BendKind::Bend => {
                let _ = write!(n.tech, "<bend><bend-alter>{peak}</bend-alter></bend>");
            }
            BendKind::PreBend => {
                let _ = write!(
                    n.tech,
                    "<bend><bend-alter>{peak}</bend-alter><pre-bend/></bend>"
                );
            }
            BendKind::BendRelease => {
                let _ = write!(
                    n.tech,
                    "<bend><bend-alter>{peak}</bend-alter></bend><bend><bend-alter>{peak}</bend-alter><release/></bend>"
                );
            }
            BendKind::PreBendRelease => {
                let _ = write!(
                    n.tech,
                    "<bend><bend-alter>{peak}</bend-alter><pre-bend/></bend><bend><bend-alter>{peak}</bend-alter><release/></bend>"
                );
            }
            BendKind::Custom => {
                w.warn("a bend curve with an unusual shape was written as a simple bend");
                if b.peak() > 0 {
                    let _ = write!(n.tech, "<bend><bend-alter>{peak}</bend-alter></bend>");
                }
            }
        }
    }
    if let Some(wh) = &tc.whammy
        && let Some(p) = wh.points.iter().max_by_key(|p| p.cents.abs())
        && p.cents != 0
    {
        let _ = write!(
            n.tech,
            "<bend><bend-alter>{}</bend-alter><with-bar>tremolo</with-bar></bend>",
            fmt_alter(p.cents)
        );
    }
    if let Some(h) = &tc.harmonic {
        let name = match h.kind {
            HarmonicKind::Natural => {
                n.tech.push_str("<harmonic><natural/></harmonic>");
                "natural"
            }
            HarmonicKind::Artificial => {
                n.tech.push_str("<harmonic><artificial/></harmonic>");
                "artificial"
            }
            HarmonicKind::Pinch => {
                n.tech.push_str("<harmonic><artificial/></harmonic>");
                "pinch"
            }
            HarmonicKind::Tapped => {
                n.tech.push_str("<harmonic><artificial/></harmonic>");
                "tapped"
            }
            HarmonicKind::Semi => {
                n.tech.push_str("<harmonic><artificial/></harmonic>");
                "semi"
            }
        };
        match h.fret {
            Some(f) => other.push(format!("harmonic:{name}:{f}")),
            None if !matches!(h.kind, HarmonicKind::Natural | HarmonicKind::Artificial) => {
                other.push(format!("harmonic:{name}"));
            }
            None => {}
        }
    }
    if tc.has(TechFlags::TAP) {
        n.tech.push_str("<tap/>");
    }
    if tc.has(TechFlags::THUMB) {
        n.tech.push_str("<pluck>p</pluck>");
    }
    for (flag, text) in [
        (TechFlags::LET_RING, "let ring"),
        (TechFlags::SLAP, "slap"),
        (TechFlags::POP, "pop"),
        (TechFlags::FADE_IN, "fade in"),
    ] {
        if tc.has(flag) {
            other.push(text.into());
        }
    }
    match tc.vibrato {
        Some(VibratoKind::Normal) => other.push("vibrato".into()),
        Some(VibratoKind::Wide) => other.push("vibrato wide".into()),
        None => {}
    }
    for o in &other {
        let _ = write!(n.tech, "<other-technical>{}</other-technical>", esc(o));
    }
    for (flag, tag) in [
        (TechFlags::ACCENT, "<accent/>"),
        (TechFlags::HEAVY_ACCENT, "<strong-accent/>"),
        (TechFlags::STACCATO, "<staccato/>"),
    ] {
        if tc.has(flag) {
            n.art.push_str(tag);
        }
    }
    if tc.tremolo_division >= 8 {
        let marks = tc.tremolo_division.trailing_zeros().saturating_sub(2);
        let _ = write!(n.orn, "<tremolo type=\"single\">{marks}</tremolo>");
    }
    match tc.strum {
        Some(StrumDirection::Up) => n.direct.push_str("<arpeggiate direction=\"up\"/>"),
        Some(StrumDirection::Down) => n.direct.push_str("<arpeggiate direction=\"down\"/>"),
        None => {}
    }
    n
}

#[allow(clippy::too_many_arguments)]
fn write_item(
    x: &mut String,
    t: &TabTrack,
    part_id: &str,
    inst: &[(u8, u8)],
    it: &Item,
    d: i64,
    voice_no: usize,
    marks: &[Marks],
    lyrics: &mut BTreeMap<i64, (String, &'static str)>,
    bar_start: i64,
    w: &mut Warnings,
) {
    for on in &it.notes {
        if on.first
            && let Some(g) = grace_xml(t, &t.notes[on.note], voice_no)
        {
            x.push_str(&g);
        }
    }
    for (k, on) in it.notes.iter().enumerate() {
        let note: &TabNote = &t.notes[on.note];
        let m = marks[on.note];
        let number = usize::from(note.string) + 1;
        let mut s = String::from("      <note");
        if note.velocity != 90 {
            let _ = write!(
                s,
                " dynamics=\"{:.2}\"",
                f64::from(note.velocity) * 100.0 / 90.0
            );
        }
        s.push('>');
        if k > 0 {
            s.push_str("<chord/>");
        }
        if t.is_percussion {
            s.push_str(
                "<unpitched><display-step>C</display-step><display-octave>5</display-octave></unpitched>",
            );
        } else if let Some(key) = t.key_of(note) {
            s.push_str(&pitch_xml(key));
        } else {
            w.warn("a note outside its instrument's neck was written with a placeholder pitch");
            s.push_str("<pitch><step>C</step><octave>4</octave></pitch>");
        }
        let _ = write!(s, "<duration>{d}</duration>");
        if on.tie_stop {
            s.push_str("<tie type=\"stop\"/>");
        }
        if on.tie_start {
            s.push_str("<tie type=\"start\"/>");
        }
        if t.is_percussion {
            let idx = inst
                .iter()
                .position(|p| *p == (note.string, note.fret))
                .unwrap_or(0);
            let _ = write!(s, "<instrument id=\"{part_id}-I{}\"/>", idx + 1);
        }
        let _ = write!(s, "<voice>{voice_no}</voice>");
        s.push_str(&type_xml(&it.notated));
        let dead = on.first && note.tech.has(TechFlags::DEAD);
        let ghost = on.first && note.tech.has(TechFlags::GHOST);
        if dead || ghost {
            let attr = if ghost { " parentheses=\"yes\"" } else { "" };
            let _ = write!(
                s,
                "<notehead{attr}>{}</notehead>",
                if dead { "x" } else { "normal" }
            );
        }
        // Notations.
        let mut n = if on.first && !t.is_percussion {
            techniques_xml(note, m, number, w)
        } else {
            Notations::default()
        };
        let mut nt = String::new();
        if k == 0 {
            nt.push_str(&tuplet_xml(it));
        }
        if on.tie_stop {
            nt.push_str("<tied type=\"stop\"/>");
        }
        if on.tie_start {
            nt.push_str("<tied type=\"start\"/>");
        }
        nt.push_str(&n.direct);
        if !n.orn.is_empty() {
            let _ = write!(nt, "<ornaments>{}</ornaments>", n.orn);
        }
        if !t.is_percussion {
            n.tech.insert_str(
                0,
                &format!("<string>{number}</string><fret>{}</fret>", note.fret),
            );
            let _ = write!(nt, "<technical>{}</technical>", n.tech);
        }
        if !n.art.is_empty() {
            let _ = write!(nt, "<articulations>{}</articulations>", n.art);
        }
        if !nt.is_empty() {
            let _ = write!(s, "<notations>{nt}</notations>");
        }
        // Lyric: on the first piece of the first note that starts at the tick.
        if k == 0
            && on.first
            && let Some((text, syl)) = lyrics.remove(&(bar_start + it.start))
        {
            let _ = write!(
                s,
                "<lyric number=\"1\"><syllabic>{syl}</syllabic><text>{}</text></lyric>",
                esc(&text)
            );
        }
        if on.first && note.tech.has(TechFlags::PALM_MUTE) {
            s.push_str("<play><mute>palm</mute></play>");
        }
        s.push_str("</note>\n");
        x.push_str(&s);
    }
}
