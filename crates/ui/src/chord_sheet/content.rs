//! The content of the sheet as drawn: the shown sheet (transposed, capo applied) turned into
//! entries with positions, for painting and for finding the chord under a click.

use super::layout::{self, Row};
use atomik_tab::chord::ChordSymbol;
use atomik_tab::sheet::{ChordSheet, SheetBlock};
use vello::kurbo::Rect;

/// Left and right margin of the content inside the pane.
pub const MARGIN: f64 = 16.0;

/// One row of a lyric or comment line, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DrawRow {
    /// Top of the row, from the top of its entry.
    pub y: f64,
    /// Height of the chord names part (0 when the row has none).
    pub chord_h: f64,
    /// Height of the lyric part (0 for a chord-only row).
    pub lyric_h: f64,
    pub row: Row,
    /// The display names of the line's chords, indexed by `Row::chords`.
    pub names: Vec<String>,
}

/// One fact of the header line (the program's words are translated when it is drawn).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Fact {
    /// The sheet's key (`Key G`).
    Key(String),
    /// The capo fret (`Capo 2`).
    Capo(u8),
    /// A `name: value` fact of the sheet, shown as written.
    Meta(String),
}

impl Fact {
    /// The text in the current language.
    pub(super) fn text(&self) -> String {
        match self {
            Fact::Key(k) => crate::i18n::tr(&format!("Key {k}")).into_owned(),
            Fact::Capo(n) => crate::i18n::tr(&format!("Capo {n}")).into_owned(),
            Fact::Meta(m) => m.clone(),
        }
    }
}

/// What an entry draws.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Kind {
    /// The title, the artist and the key/capo line.
    Title {
        title: String,
        artist: String,
        info: Vec<Fact>,
    },
    /// A section header.
    Section(String),
    /// Lyric lines and chord-only lines.
    Lines(Vec<DrawRow>),
    /// A comment (wrapped like a lyric, drawn muted).
    Comment(Vec<DrawRow>),
    /// A block of tab text, one string per line.
    Tab(Vec<String>),
    /// An empty line.
    Blank,
}

/// An entry: a block with its place.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Entry {
    pub y: f64,
    pub h: f64,
    pub kind: Kind,
}

/// A laid-out sheet.
#[derive(Debug, Clone, Default)]
pub(super) struct Content {
    pub entries: Vec<Entry>,
    pub height: f64,
    /// `(y of the line's top, linked bar)` in order of appearance.
    pub anchors: Vec<(f64, u32)>,
    /// Where each chord name is: its rectangle (content x from the left margin, y from the top)
    /// and the chord.
    pub chord_hits: Vec<(Rect, ChordSymbol)>,
    /// The distinct chords in order of first appearance.
    pub chords: Vec<ChordSymbol>,
    /// The height of one lyric line with its chords: the unit of lines per minute.
    pub line_unit: f64,
}

/// The heights of the parts of a line at lyric size `size`.
pub(super) fn chord_row_height(size: f32) -> f64 {
    f64::from(size) * 1.3
}

/// The height of a lyric row.
pub(super) fn lyric_row_height(size: f32) -> f64 {
    f64::from(size) * 1.35
}

/// Lays `sheet` out at lyric size `size` in rows no wider than `width`.
pub(super) fn build(sheet: &ChordSheet, size: f32, width: f64) -> Content {
    let mut out = Content {
        line_unit: chord_row_height(size) + lyric_row_height(size),
        ..Content::default()
    };
    let s = f64::from(size);
    let mut y = 6.0;
    // Header.
    let mut info = Vec::new();
    if let Some(k) = sheet.key {
        info.push(Fact::Key(k.name()));
    }
    if sheet.capo > 0 {
        info.push(Fact::Capo(sheet.capo));
    }
    for (name, value) in &sheet.meta {
        info.push(Fact::Meta(format!("{name}: {value}")));
    }
    let header_h = s * 1.9
        + if sheet.artist.is_empty() {
            0.0
        } else {
            s * 1.3
        }
        + if info.is_empty() { 0.0 } else { s * 1.3 }
        + s * 0.7;
    if !sheet.title.is_empty() || !sheet.artist.is_empty() || !info.is_empty() {
        out.entries.push(Entry {
            y,
            h: header_h,
            kind: Kind::Title {
                title: sheet.title.clone(),
                artist: sheet.artist.clone(),
                info,
            },
        });
        y += header_h;
    }
    for block in &sheet.blocks {
        match block {
            SheetBlock::Section(name) => {
                let h = s * 2.1;
                out.entries.push(Entry {
                    y: y + s * 0.5,
                    h: h - s * 0.5,
                    kind: Kind::Section(name.clone()),
                });
                y += h;
            }
            SheetBlock::Blank => {
                out.entries.push(Entry {
                    y,
                    h: s * 0.8,
                    kind: Kind::Blank,
                });
                y += s * 0.8;
            }
            SheetBlock::Comment(text) => {
                let rows = draw_rows(text, &[], size, width, &mut Vec::new());
                let h = rows.last().map_or(0.0, |r| r.y + r.lyric_h) + s * 0.3;
                out.entries.push(Entry {
                    y,
                    h,
                    kind: Kind::Comment(rows),
                });
                y += h;
            }
            SheetBlock::Tab(t) => {
                let h = t.lines.len() as f64 * s * 1.15 + s * 0.6;
                out.entries.push(Entry {
                    y,
                    h,
                    kind: Kind::Tab(t.lines.clone()),
                });
                y += h;
            }
            SheetBlock::Line { text, chords, bar } => {
                let named: Vec<(usize, String)> =
                    chords.iter().map(|(i, c)| (*i, c.name())).collect();
                let mut hits = Vec::new();
                let rows = draw_rows(text, &named, size, width, &mut hits);
                let h = rows.last().map_or(0.0, |r| r.y + r.chord_h + r.lyric_h) + s * 0.25;
                for (rect, k) in hits {
                    out.chord_hits.push((
                        Rect::new(rect.x0, rect.y0 + y, rect.x1, rect.y1 + y),
                        chords[k].1.clone(),
                    ));
                }
                for (_, c) in chords {
                    if !out.chords.contains(c) {
                        out.chords.push(c.clone());
                    }
                }
                if let Some(b) = bar {
                    out.anchors.push((y, *b));
                }
                out.entries.push(Entry {
                    y,
                    h,
                    kind: Kind::Lines(rows),
                });
                y += h;
            }
        }
    }
    out.height = y + s * 2.0;
    out
}

/// The rows of one line; `hits` collects the chord rectangles (x from the left margin, y from
/// the top of the line) with the index of the chord in the line.
fn draw_rows(
    text: &str,
    named: &[(usize, String)],
    size: f32,
    width: f64,
    hits: &mut Vec<(Rect, usize)>,
) -> Vec<DrawRow> {
    let lyric = !text.trim().is_empty();
    let rows = layout::layout_line(text, named, size, width);
    let ch = chord_row_height(size);
    let lh = lyric_row_height(size);
    let mut y = 0.0;
    let mut out = Vec::new();
    for row in rows {
        let chord_h = if row.chords.is_empty() { 0.0 } else { ch };
        let lyric_h = if lyric || row.chords.is_empty() {
            lh
        } else {
            0.0
        };
        for (x, k) in &row.chords {
            let w = layout::chord_width(&named[*k].1, size);
            hits.push((Rect::new(*x, y, x + w, y + chord_h), *k));
        }
        let names = named.iter().map(|(_, n)| n.clone()).collect();
        out.push(DrawRow {
            y,
            chord_h,
            lyric_h,
            row,
            names,
        });
        y += chord_h + lyric_h;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_tab::sheet::parse_sheet;

    fn sheet() -> ChordSheet {
        parse_sheet("{title: Made Up}\n{artist: Nobody}\n\n[Verse]\n[Am]la la [F]la la\n\n[Chorus]\n[C]hey [G]ho\n")
            .expect("a made-up sheet reads")
            .sheet
    }

    #[test]
    fn the_sheet_becomes_a_header_sections_and_lines_in_order() {
        let c = build(&sheet(), 14.0, 600.0);
        let kinds: Vec<&str> = c
            .entries
            .iter()
            .map(|e| match &e.kind {
                Kind::Title { .. } => "title",
                Kind::Section(_) => "section",
                Kind::Lines(_) => "lines",
                Kind::Comment(_) => "comment",
                Kind::Tab(_) => "tab",
                Kind::Blank => "blank",
            })
            .collect();
        assert_eq!(kinds[0], "title");
        assert!(kinds.contains(&"section") && kinds.contains(&"lines"));
        assert!(
            c.entries
                .windows(2)
                .all(|w| w[1].y >= w[0].y + w[0].h - 1e-6)
        );
        assert!(c.height > c.entries.last().map_or(0.0, |e| e.y + e.h));
    }

    #[test]
    fn every_chord_has_one_hit_rectangle_and_the_distinct_chords_are_listed() {
        let c = build(&sheet(), 14.0, 600.0);
        assert_eq!(c.chord_hits.len(), 4);
        let names: Vec<String> = c.chords.iter().map(ChordSymbol::name).collect();
        assert_eq!(names, vec!["Am", "F", "C", "G"]);
        for (r, _) in &c.chord_hits {
            assert!(r.width() > 0.0 && r.height() > 0.0);
        }
    }

    #[test]
    fn a_line_linked_to_a_bar_is_an_anchor() {
        let mut s = sheet();
        for b in &mut s.blocks {
            if let SheetBlock::Line { bar, .. } = b {
                *bar = Some(5);
                break;
            }
        }
        let c = build(&s, 14.0, 600.0);
        assert_eq!(c.anchors.len(), 1);
        assert_eq!(c.anchors[0].1, 5);
    }

    #[test]
    fn a_narrower_pane_makes_a_taller_sheet() {
        let wide = build(&sheet(), 14.0, 800.0);
        let narrow = build(&sheet(), 14.0, 40.0);
        assert!(narrow.height > wide.height);
    }
}
