//! The pure layout of a chord sheet: where the chord names and the pieces of a lyric line go.
//!
//! A line is a text with chords anchored to its characters. Text widths cannot be measured here
//! (the same layout must serve painting and hit-testing, and recorded lists must not depend on
//! a font), so every character advances by a fixed share of the font size and a lyric is drawn
//! as **segments**, one per stretch between two anchors, each placed by this layout. A chord
//! always sits over the first character of its segment, whatever the font does inside the
//! segment.
//!
//! * **Overlap-safe.** When a chord name would run into the one before it, the chord and the
//!   rest of the line move right (the lyric gets a gap), so every name stays over its syllable
//!   and none touches another.
//! * **Wrapping.** A line wider than the pane breaks after a space (inside a word only when a
//!   word is wider than the pane); a chord stays with the segment it is anchored to.
//! * **Chord-only lines** (no text, chords at columns) lay out the same way and draw no lyric.

/// The advance of one lyric character, as a share of the font size.
pub const LYRIC_ADVANCE: f64 = 0.5;
/// The advance of one character of a chord name (drawn bold and a little larger).
pub const CHORD_ADVANCE: f64 = 0.62;
/// The size of a chord name as a share of the lyric size.
pub const CHORD_SCALE: f64 = 0.92;

/// The width of a chord name at lyric size `size`.
pub fn chord_width(name: &str, size: f32) -> f64 {
    name.chars().count() as f64 * f64::from(size) * CHORD_SCALE * CHORD_ADVANCE
}

/// The width of `text` at lyric size `size`.
pub fn text_width(text: &str, size: f32) -> f64 {
    text.chars().count() as f64 * f64::from(size) * LYRIC_ADVANCE
}

/// One drawn row of a line: chords over lyric pieces, x relative to the row's left edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// `(x, index into the line's chords)`.
    pub chords: Vec<(f64, usize)>,
    /// `(x, text)`: the lyric pieces (never blank, never trailing spaces).
    pub segs: Vec<(f64, String)>,
}

/// Lays a line out in rows no wider than `width`. `chords` are `(character index, name)` in
/// ascending index order.
pub fn layout_line(text: &str, chords: &[(usize, String)], size: f32, width: f64) -> Vec<Row> {
    let adv = f64::from(size) * LYRIC_ADVANCE;
    let mut chars: Vec<char> = text.chars().collect();
    let total = chords
        .iter()
        .map(|(i, _)| i + 1)
        .max()
        .unwrap_or(0)
        .max(chars.len());
    chars.resize(total, ' ');
    if total == 0 {
        return vec![Row {
            chords: Vec::new(),
            segs: Vec::new(),
        }];
    }
    // x of every character start, with the gaps the chords push in; x of every chord.
    let mut xs = vec![0.0; total + 1];
    let mut chord_x = vec![0.0; chords.len()];
    let mut extra = 0.0;
    let mut prev_end = f64::NEG_INFINITY;
    let mut next = 0usize;
    for (i, slot) in xs.iter_mut().enumerate() {
        while next < chords.len() && chords[next].0 == i {
            let desired = i as f64 * adv + extra;
            let x = desired.max(prev_end + adv);
            extra += x - desired;
            chord_x[next] = x;
            prev_end = x + chord_width(&chords[next].1, size);
            next += 1;
        }
        *slot = i as f64 * adv + extra;
    }
    // Where each chord's name ends, by the character it is anchored to.
    let chord_end = |k: usize| chord_x[k] + chord_width(&chords[k].1, size);
    let mut rows = Vec::new();
    let mut start = 0usize;
    while start < total {
        // The longest run of characters that fits.
        let mut end = start + 1;
        let first_chord = chords.partition_point(|(i, _)| *i < start);
        let mut ck = first_chord;
        while end < total {
            // The right edge once `end` joins the row.
            let mut edge = xs[end] + adv;
            let mut k = ck;
            while k < chords.len() && chords[k].0 <= end {
                edge = edge.max(chord_end(k));
                k += 1;
            }
            if edge - xs[start] > width {
                break;
            }
            ck = k;
            end += 1;
        }
        if end < total {
            // Break after the last space of the run, when there is one.
            if let Some(b) = (start + 1..=end).rev().find(|b| chars[b - 1] == ' ') {
                end = b;
            }
        }
        rows.push(make_row(&chars, chords, &xs, &chord_x, start, end));
        start = end;
    }
    rows
}

fn make_row(
    chars: &[char],
    chords: &[(usize, String)],
    xs: &[f64],
    chord_x: &[f64],
    start: usize,
    end: usize,
) -> Row {
    let lo = chords.partition_point(|(i, _)| *i < start);
    let hi = chords.partition_point(|(i, _)| *i < end);
    // Chords stacked on the first character sit left of it: the row starts at the first.
    let base = if lo < hi && chords[lo].0 == start {
        chord_x[lo].min(xs[start])
    } else {
        xs[start]
    };
    let row_chords: Vec<(f64, usize)> = (lo..hi)
        .map(|k| ((chord_x[k] - base).max(0.0), k))
        .collect();
    // Pieces: between the anchors of this row.
    let mut cuts: Vec<usize> = vec![start];
    cuts.extend(
        chords[lo..hi]
            .iter()
            .map(|(i, _)| *i)
            .filter(|i| *i > start),
    );
    cuts.dedup();
    cuts.push(end);
    let mut segs = Vec::new();
    for w in cuts.windows(2) {
        let piece: String = chars[w[0]..w[1]].iter().collect();
        let piece = piece.trim_end();
        if !piece.trim().is_empty() {
            segs.push((xs[w[0]] - base, piece.to_owned()));
        }
    }
    Row {
        chords: row_chords,
        segs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(v: &[(usize, &str)]) -> Vec<(usize, String)> {
        v.iter().map(|(i, n)| (*i, (*n).to_owned())).collect()
    }

    #[test]
    fn a_chord_sits_over_the_character_it_is_anchored_to() {
        let rows = layout_line("la la la", &named(&[(0, "Am"), (6, "F")]), 10.0, 1000.0);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.chords.len(), 2);
        assert!((r.chords[0].0 - 0.0).abs() < 1e-9);
        // The second chord is over the seventh character, and the second lyric piece starts there.
        assert!((r.chords[1].0 - 6.0 * 10.0 * LYRIC_ADVANCE).abs() < 1e-9);
        assert_eq!(r.segs[0].1, "la la");
        assert_eq!(r.segs[1].1, "la");
        assert!((r.segs[1].0 - r.chords[1].0).abs() < 1e-9);
    }

    #[test]
    fn a_chord_that_would_touch_the_one_before_pushes_the_rest_of_the_line_right() {
        // "Cmaj7" is wider than the two characters before the next anchor.
        let rows = layout_line("ab cd", &named(&[(0, "Cmaj7"), (2, "G")]), 10.0, 1000.0);
        let r = &rows[0];
        let end_of_first = r.chords[0].0 + chord_width("Cmaj7", 10.0);
        assert!(
            r.chords[1].0 >= end_of_first + 10.0 * LYRIC_ADVANCE - 1e-9,
            "{r:?}"
        );
        // The lyric moved with the chord: its piece starts under it.
        assert!((r.segs[1].0 - r.chords[1].0).abs() < 1e-9);
    }

    #[test]
    fn chords_never_overlap_however_many_share_a_character() {
        let chords = named(&[(0, "A"), (0, "B"), (0, "C"), (1, "D")]);
        let r = &layout_line("xy", &chords, 12.0, 1000.0)[0];
        for w in r.chords.windows(2) {
            let end = w[0].0 + chord_width(&chords[w[0].1].1, 12.0);
            assert!(w[1].0 >= end, "{r:?}");
        }
    }

    #[test]
    fn a_long_line_wraps_after_a_space_and_keeps_each_chord_with_its_piece() {
        let text = "one two three four five six";
        let chords = named(&[(0, "C"), (8, "G"), (19, "Am")]);
        let rows = layout_line(text, &chords, 10.0, 90.0);
        assert!(rows.len() > 1, "{rows:?}");
        // Every chord is placed exactly once, and over a piece that starts at its x.
        let all: Vec<usize> = rows
            .iter()
            .flat_map(|r| r.chords.iter().map(|c| c.1))
            .collect();
        assert_eq!(all, vec![0, 1, 2]);
        for r in &rows {
            for (x, _) in &r.chords {
                assert!(r.segs.iter().any(|(sx, _)| (sx - x).abs() < 1e-9));
            }
            for (x, s) in &r.segs {
                assert!(x + text_width(s, 10.0) <= 90.0 + 1e-6, "{r:?}");
            }
        }
        // The text read back is the line, up to the spaces at the breaks.
        let joined: String = rows
            .iter()
            .flat_map(|r| r.segs.iter().map(|s| s.1.clone()))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(joined.replace("  ", " "), text);
    }

    #[test]
    fn a_word_wider_than_the_pane_is_cut() {
        let rows = layout_line("abcdefghijklmnopqrstuvwxyz", &[], 10.0, 60.0);
        assert!(rows.len() > 2);
        assert!(rows.iter().all(|r| r.segs.len() == 1));
    }

    #[test]
    fn a_chord_only_line_has_chords_and_no_lyric() {
        let rows = layout_line("", &named(&[(0, "Am"), (8, "F"), (16, "C")]), 10.0, 1000.0);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].segs.is_empty());
        assert_eq!(rows[0].chords.len(), 3);
        assert!(rows[0].chords[2].0 > rows[0].chords[1].0);
    }

    #[test]
    fn an_empty_line_is_one_empty_row() {
        let rows = layout_line("", &[], 10.0, 100.0);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].chords.is_empty() && rows[0].segs.is_empty());
    }

    #[test]
    fn a_tiny_pane_still_terminates_with_every_character_placed() {
        let rows = layout_line("hello world", &named(&[(0, "C"), (6, "G")]), 10.0, 1.0);
        let text: String = rows
            .iter()
            .flat_map(|r| r.segs.iter().map(|s| s.1.clone()))
            .collect();
        assert_eq!(text.replace(' ', ""), "helloworld");
    }
}
