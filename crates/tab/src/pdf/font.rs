//! The two PDF fonts the printouts use and their metrics.
//!
//! The pages use the standard Helvetica and Helvetica-Bold fonts of the PDF format: every viewer
//! and printer has them, so nothing is embedded and the file stays a few kilobytes. The price is the
//! character set: text is written in WinAnsi (Latin-1 plus the usual typographic punctuation) and a
//! character outside it prints as `?`. The sharp and flat signs print as `#` and `b`, which is
//! also how chord names are written everywhere else in the program.
//!
//! Layout has to know how wide a text is, so the advance widths of the printable ASCII range are
//! tables here (units of 1/1000 of the font size, the published metrics of the two fonts); the Latin-1
//! letters are estimated from the width of a plain letter, which is within a few percent.

/// A font of the printout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    /// Helvetica.
    Regular,
    /// Helvetica-Bold.
    Bold,
    /// Courier, for tab blocks whose columns must line up.
    Mono,
}

impl Font {
    /// The PostScript name written to the file.
    pub fn base_name(self) -> &'static [u8] {
        match self {
            Font::Regular => b"Helvetica",
            Font::Bold => b"Helvetica-Bold",
            Font::Mono => b"Courier",
        }
    }
}

/// Advance widths of the characters 32 to 126 in Helvetica.
#[rustfmt::skip]
const REGULAR: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

/// Advance widths of the characters 32 to 126 in Helvetica-Bold.
#[rustfmt::skip]
const BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611,
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// The WinAnsi byte of a character, `?` when the encoding has none. The accidentals become the
/// letters chord names use.
pub fn encode_char(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\u{a0}'..='\u{ff}' => c as u32 as u8,
        '\u{266f}' => b'#',
        '\u{266d}' => b'b',
        '\u{20ac}' => 0x80,
        '\u{2026}' => 0x85,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201c}' => 0x93,
        '\u{201d}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{2122}' => 0x99,
        '\t' => b' ',
        _ => b'?',
    }
}

/// The text as WinAnsi bytes.
pub fn encode(text: &str) -> Vec<u8> {
    text.chars().map(encode_char).collect()
}

/// The advance of one WinAnsi byte in 1/1000 em.
fn advance(font: Font, b: u8) -> u16 {
    let table = match font {
        Font::Regular => &REGULAR,
        Font::Bold => &BOLD,
        Font::Mono => return 600,
    };
    match b {
        32..=126 => table[usize::from(b - 32)],
        0xa0 => 278,
        0xc0..=0xde => 700,
        0xdf => 611,
        0xe0..=0xff => 540,
        0x85 | 0x97 => 1000,
        0x91 | 0x92 => 222,
        0x93 | 0x94 => 333,
        0x95 => 350,
        _ => 556,
    }
}

/// The width in points of `text` at `size` points.
pub fn text_width(font: Font, size: f32, text: &str) -> f32 {
    let units: u32 = encode(text)
        .into_iter()
        .map(|b| u32::from(advance(font, b)))
        .sum();
    units as f32 * size / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_follow_the_published_metrics() {
        assert!((text_width(Font::Regular, 10.0, "i") - 2.22).abs() < 1e-4);
        assert!((text_width(Font::Regular, 10.0, "W") - 9.44).abs() < 1e-4);
        assert!(text_width(Font::Bold, 10.0, "0") > 5.0);
        assert_eq!(text_width(Font::Bold, 12.0, ""), 0.0);
    }

    #[test]
    fn accidentals_print_as_letters_and_the_unknown_as_a_question_mark() {
        assert_eq!(encode("F\u{266f}m B\u{266d}"), b"F#m Bb");
        assert_eq!(encode("\u{4e2d}"), b"?");
        assert_eq!(encode("caf\u{e9}"), vec![b'c', b'a', b'f', 0xe9]);
    }
}
