//! The chord sheet printout.
//!
//! Each lyric line is cut into *atoms* (a word with its trailing space, or the tail of a word after
//! a chord), a chord belongs to the atom that starts at its anchor and sits over its first character.
//! Atoms are laid along the row: a chord that would touch the one before pushes its atom (and the
//! lyric after it) to the right, and a row that is full wraps at the next atom, with the chord kept
//! over the same word. A row is the chord line (when it has chords) and the lyric line, and is
//! never split by a page break.

use super::diagram::{diagram_height, draw_diagram, start_frets_of};
use super::draw::{Anchor, Ink};
use super::font::{Font, text_width};
use super::{Layout, PageSize, Pager, PdfError, wrap};
use crate::chord::ChordSymbol;
use crate::instrument::{Capo, Family, find_instrument};
use crate::sheet::{ChordSheet, SheetBlock};
use crate::transpose::Spelling;
use crate::voicing::{VoicingCache, VoicingOptions};

/// A strip of chord diagrams at the top of the sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetDiagrams {
    /// The catalogue id of the instrument (`guitar6`, `ukulele_soprano`, `banjo5`); its default
    /// tuning is used.
    pub instrument_id: String,
}

/// What a chord sheet printout looks like.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetPdfOptions {
    /// The paper.
    pub page: PageSize,
    /// The margin in points (clamped to 20..=100).
    pub margin: f32,
    /// The lyric size in points (clamped to 7..=24).
    pub font_size: f32,
    /// Semitones to transpose by (the chords sound that much higher).
    pub transpose: i32,
    /// The capo the shapes are written for; `None` keeps the sheet's own. The chords are
    /// re-written as the shapes for that capo, so the sound does not change.
    pub capo: Option<u8>,
    /// Sharps or flats for the transposed chords; `None` lets the key (or the sheet's habit) decide.
    pub spelling: Option<Spelling>,
    /// The diagram strip, when wanted.
    pub diagrams: Option<SheetDiagrams>,
}

impl Default for SheetPdfOptions {
    fn default() -> Self {
        Self {
            page: PageSize::A4,
            margin: 48.0,
            font_size: 11.0,
            transpose: 0,
            capo: None,
            spelling: None,
            diagrams: None,
        }
    }
}

/// The widest gap kept between two chord names.
const CHORD_GAP: f32 = 4.0;
const DIA_W: f32 = 36.0;
const DIA_MAX: usize = 28;

struct Atom {
    text: String,
    chord: Option<String>,
}

/// The atoms of a lyric line: the chord at character `at` goes with the atom that starts there.
fn atoms(text: &str, chords: &[(usize, ChordSymbol)]) -> Vec<Atom> {
    let chars: Vec<char> = text.chars().collect();
    let mut sorted: Vec<&(usize, ChordSymbol)> = chords.iter().collect();
    sorted.sort_by_key(|c| c.0);
    let mut out: Vec<Atom> = Vec::new();
    let mut ci = 0;
    let mut pos = 0;
    let mut cur = String::new();
    let mut cur_chord: Option<String> = None;
    let flush = |cur: &mut String, chord: &mut Option<String>, out: &mut Vec<Atom>| {
        if !cur.is_empty() || chord.is_some() {
            out.push(Atom {
                text: std::mem::take(cur),
                chord: chord.take(),
            });
        }
    };
    loop {
        // A chord anchored here starts a new atom (several at once: all but the last are
        // chord-only atoms).
        while ci < sorted.len() && sorted[ci].0 <= pos {
            flush(&mut cur, &mut cur_chord, &mut out);
            cur_chord = Some(sorted[ci].1.name());
            ci += 1;
        }
        if pos >= chars.len() {
            break;
        }
        let c = chars[pos];
        cur.push(c);
        pos += 1;
        // A word ends after its trailing spaces.
        if c == ' ' && chars.get(pos).is_some_and(|n| *n != ' ') {
            flush(&mut cur, &mut cur_chord, &mut out);
        }
    }
    flush(&mut cur, &mut cur_chord, &mut out);
    out
}

/// One wrapped row: the atoms with their x (relative to the left margin).
struct Row {
    items: Vec<(f32, String, Option<String>)>,
}

fn place(atoms: Vec<Atom>, size: f32, chord_size: f32, width: f32) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut cur = Row { items: Vec::new() };
    let mut cursor = 0.0f32;
    let mut chord_end = f32::MIN;
    for a in atoms {
        let tw = text_width(Font::Regular, size, &a.text);
        let cw = a
            .chord
            .as_ref()
            .map_or(0.0, |c| text_width(Font::Bold, chord_size, c));
        let mut x = cursor;
        if a.chord.is_some() {
            x = x.max(chord_end + CHORD_GAP);
        }
        let visible = tw.max(cw);
        if !cur.items.is_empty() && x + visible > width {
            rows.push(std::mem::replace(&mut cur, Row { items: Vec::new() }));
            x = 0.0;
            chord_end = f32::MIN;
        }
        cursor = x + tw;
        if a.chord.is_some() {
            chord_end = x + cw;
        }
        cur.items.push((x, a.text, a.chord));
    }
    if !cur.items.is_empty() {
        rows.push(cur);
    }
    rows
}

/// Lays a chord sheet out on pages (see the module docs).
pub fn layout_sheet(sheet: &ChordSheet, opts: &SheetPdfOptions) -> Result<Layout, PdfError> {
    let meaningful = sheet.blocks.iter().any(|b| {
        !matches!(b, SheetBlock::Blank)
            && match b {
                SheetBlock::Line { text, chords, .. } => {
                    !text.trim().is_empty() || !chords.is_empty()
                }
                _ => true,
            }
    });
    if !meaningful {
        return Err(PdfError::Empty("the chord sheet is empty"));
    }
    let mut shown = sheet.clone();
    let capo = opts.capo.map_or(sheet.capo, |c| c.min(24));
    let delta = opts.transpose.clamp(-11, 11) + i32::from(sheet.capo) - i32::from(capo);
    if delta != 0 {
        shown.transpose(delta, opts.spelling);
    }
    shown.capo = capo;

    let margin = if opts.margin.is_finite() {
        opts.margin.clamp(20.0, 100.0)
    } else {
        48.0
    };
    let size = if opts.font_size.is_finite() {
        opts.font_size.clamp(7.0, 24.0)
    } else {
        11.0
    };
    let chord_size = size * 0.95;
    let mut p = Pager::new(opts.page.points(), margin);
    let (l, w) = (p.left(), p.width());

    // Header.
    if !shown.title.trim().is_empty() {
        for line in wrap(Font::Bold, 18.0, &shown.title, w) {
            p.text(
                l,
                p.y + 14.0,
                18.0,
                Font::Bold,
                Ink::Black,
                Anchor::Start,
                &line,
            );
            p.y += 21.0;
        }
    }
    if !shown.artist.trim().is_empty() {
        p.text(
            l,
            p.y + 10.0,
            11.0,
            Font::Regular,
            Ink::Dark,
            Anchor::Start,
            &shown.artist,
        );
        p.y += 15.0;
    }
    let mut facts: Vec<String> = Vec::new();
    if let Some(k) = shown.key {
        facts.push(format!("Key: {}", k.name()));
    }
    if shown.capo > 0 {
        facts.push(format!("Capo: {}", shown.capo));
    }
    for (name, v) in &shown.meta {
        if !v.trim().is_empty() {
            let mut n = name.clone();
            if let Some(f) = n.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            facts.push(format!("{n}: {}", v.trim()));
        }
    }
    if !facts.is_empty() {
        p.text(
            l,
            p.y + 8.0,
            8.5,
            Font::Regular,
            Ink::Mid,
            Anchor::Start,
            &facts.join("   \u{b7}   "),
        );
        p.y += 13.0;
    }
    if p.y > p.margin {
        p.y += 4.0;
    }

    // The diagram strip.
    if let Some(d) = &opts.diagrams {
        strip(&mut p, &shown, &d.instrument_id);
    }

    // Body.
    for b in &shown.blocks {
        if p.full {
            break;
        }
        match b {
            SheetBlock::Blank => p.y += size * 0.7,
            SheetBlock::Section(name) => {
                let h = size * 1.5;
                // Keep a heading with the lines after it.
                p.ensure(h + size * 3.4);
                if p.y > p.margin + 0.5 {
                    p.y += size * 0.5;
                }
                p.text(
                    l,
                    p.y + size * 1.05,
                    size * 1.05,
                    Font::Bold,
                    Ink::Black,
                    Anchor::Start,
                    name,
                );
                p.y += h;
            }
            SheetBlock::Comment(c) => {
                for line in wrap(Font::Regular, size * 0.92, c, w) {
                    p.ensure(size * 1.3);
                    p.text(
                        l,
                        p.y + size,
                        size * 0.92,
                        Font::Regular,
                        Ink::Dark,
                        Anchor::Start,
                        &line,
                    );
                    p.y += size * 1.3;
                }
            }
            SheetBlock::Tab(t) => {
                let widest = t
                    .lines
                    .iter()
                    .map(|s| text_width(Font::Mono, 1.0, s))
                    .fold(0.0f32, f32::max);
                let tsize = if widest > 0.0 {
                    (size * 0.85).min(w / widest).max(4.5)
                } else {
                    size * 0.85
                };
                let lh = tsize * 1.2;
                p.ensure(lh * t.lines.len().min(8) as f32);
                for line in &t.lines {
                    p.ensure(lh);
                    p.text(
                        l,
                        p.y + tsize,
                        tsize,
                        Font::Mono,
                        Ink::Black,
                        Anchor::Start,
                        line,
                    );
                    p.y += lh;
                }
                p.y += size * 0.3;
            }
            SheetBlock::Line { text, chords, .. } => {
                let rows = place(atoms(text, chords), size, chord_size, w);
                let rows = if rows.is_empty() {
                    vec![Row { items: Vec::new() }]
                } else {
                    rows
                };
                for row in rows {
                    let has_chords = row.items.iter().any(|i| i.2.is_some());
                    let chord_h = if has_chords { size * 1.2 } else { 0.0 };
                    let lyric_h = size * 1.35;
                    p.ensure(chord_h + lyric_h);
                    if has_chords {
                        for (x, _, c) in &row.items {
                            if let Some(c) = c {
                                p.text(
                                    l + x,
                                    p.y + size * 0.95,
                                    chord_size,
                                    Font::Bold,
                                    Ink::Black,
                                    Anchor::Start,
                                    c,
                                );
                            }
                        }
                    }
                    for (x, t, _) in &row.items {
                        let t = t.trim_end();
                        if !t.is_empty() {
                            p.text(
                                l + x,
                                p.y + chord_h + size * 1.0,
                                size,
                                Font::Regular,
                                Ink::Black,
                                Anchor::Start,
                                t,
                            );
                        }
                    }
                    p.y += chord_h + lyric_h;
                }
            }
        }
    }
    let title = shown.title.clone();
    let author = shown.artist.clone();
    Ok(p.finish(&title, &author, Vec::new()))
}

/// Rows of diagrams for the chords the sheet uses.
fn strip(p: &mut Pager, sheet: &ChordSheet, instrument_id: &str) {
    let Some(inst) = find_instrument(instrument_id) else {
        return;
    };
    let tuning = inst.default_tuning().tuning.clone();
    let starts = start_frets_of(&tuning, Capo::NONE);
    let opts = if inst.family == Family::Bass {
        VoicingOptions::bass()
    } else {
        VoicingOptions::default()
    };
    let mut cache = VoicingCache::new();
    let mut entries = Vec::new();
    for c in sheet.chords_used().into_iter().take(DIA_MAX) {
        if let Some(v) = cache.get(&tuning, Capo::NONE, &c, &opts).first() {
            entries.push((c.name(), v.clone()));
        }
    }
    if entries.is_empty() {
        return;
    }
    let gap = 8.0;
    let per_row = (((p.width() + gap) / (DIA_W + gap)).floor() as usize).max(1);
    let h = diagram_height(DIA_W, true);
    let l = p.left();
    p.text(
        l,
        p.y + 7.0,
        7.5,
        Font::Bold,
        Ink::Mid,
        Anchor::Start,
        &format!("Chords for {}", inst.name),
    );
    p.y += 11.0;
    for row in entries.chunks(per_row) {
        p.ensure(h + 8.0);
        let y = p.y;
        for (i, (name, v)) in row.iter().enumerate() {
            let x = l + i as f32 * (DIA_W + gap);
            draw_diagram(p.page(), (x, y), DIA_W, Some(name), v, &starts);
        }
        p.y += h + 8.0;
    }
    let (l, r) = (p.left(), p.right());
    let y = p.y;
    p.page().line((l, y), (r, y), 0.4, Ink::Light);
    p.y += 10.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str) -> ChordSymbol {
        ChordSymbol::parse(name).expect("a chord")
    }

    #[test]
    fn a_chord_goes_with_the_atom_that_starts_at_its_anchor() {
        let a = atoms("hello world", &[(0, c("G")), (6, c("D"))]);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].text, "hello ");
        assert_eq!(a[0].chord.as_deref(), Some("G"));
        assert_eq!(a[1].text, "world");
        assert_eq!(a[1].chord.as_deref(), Some("D"));
    }

    #[test]
    fn a_chord_inside_a_word_splits_it() {
        let a = atoms("morning", &[(0, c("Am")), (4, c("C"))]);
        assert_eq!(a[0].text, "morn");
        assert_eq!(a[1].text, "ing");
    }

    #[test]
    fn chords_that_would_touch_are_pushed_apart() {
        let rows = place(
            atoms("ab", &[(0, c("Gmaj7")), (1, c("Dsus4"))]),
            11.0,
            10.5,
            400.0,
        );
        let items = &rows[0].items;
        let first_end = items[0].0 + text_width(Font::Bold, 10.5, "Gmaj7");
        assert!(items[1].0 >= first_end + CHORD_GAP - 0.01);
    }

    #[test]
    fn a_long_line_wraps_at_an_atom() {
        let text = "one two three four five six seven eight nine ten";
        let rows = place(atoms(text, &[(0, c("C")), (24, c("G"))]), 11.0, 10.5, 90.0);
        assert!(rows.len() > 1);
        assert!(rows.iter().all(|r| !r.items.is_empty()));
    }
}
