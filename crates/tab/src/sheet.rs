//! Chord sheets: lyrics with chords, read from and written to plain text.
//!
//! A [`ChordSheet`] is a list of [`SheetBlock`]s. A lyric line keeps its chords as
//! `(character index, chord)` pairs, so a chord belongs to the syllable under it whatever the
//! font or the wrapping (a chord index past the end of the text is a chord after the last
//! lyric, or a chord-only line). Three text forms are read:
//!
//! * **chords over lyrics**: a line where at least 60 % of the tokens read as chords sits above
//!   a lyric line, each chord anchored to the character under its first letter. A chord line
//!   with no lyric line below it is a chord-only line. Bars, repeat counts and `N.C.` are
//!   ignored when counting.
//! * **inline brackets** (`[Am]lyrics`), with the `{title:}`, `{artist:}`, `{key:}`, `{capo:}`,
//!   `{comment:}` and `{start_of_chorus}` style directives and `{start_of_tab}` blocks.
//! * **section headers**, `[Verse 1]`, `[Chorus]` or `Verse 1:`, in either form.
//!
//! [`parse_sheet`] tells the two forms apart. Both can be written back
//! ([`ChordSheet::to_over_lyrics`], [`ChordSheet::to_inline`]). Bar anchors (the link of a line
//! to a bar of the song, used to follow the transport) live in [`SheetBlock::Line::bar`] and are
//! part of the project, not of the text forms.
//!
//! Parsing is total: any text gives a sheet. Only the size limits fail ([`SheetError`]): at most
//! [`MAX_SHEET_BYTES`] bytes and [`MAX_LINES`] lines; a line is cut at [`MAX_LINE_CHARS`]
//! characters and keeps at most [`MAX_CHORDS_PER_LINE`] chords (a warning says so).

use crate::chord::ChordSymbol;
use crate::transpose::{Key, Spelling, transpose_chord, transpose_key};

/// The most bytes a sheet text may have.
pub const MAX_SHEET_BYTES: usize = 2 * 1024 * 1024;
/// The most lines a sheet text may have.
pub const MAX_LINES: usize = 20_000;
/// The most characters kept of one line.
pub const MAX_LINE_CHARS: usize = 1_000;
/// The most chords kept on one line.
pub const MAX_CHORDS_PER_LINE: usize = 64;

/// Why a sheet could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SheetError {
    /// The text is bigger than the limit.
    #[error("the chord sheet is too large ({0} bytes, at most {MAX_SHEET_BYTES})")]
    TooLarge(usize),
    /// The text has too many lines.
    #[error("the chord sheet has too many lines (at most {MAX_LINES})")]
    TooManyLines,
}

/// A block of a tab drawn as text inside a sheet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TabBlockText {
    /// The lines, verbatim.
    pub lines: Vec<String>,
}

/// One block of a sheet.
#[derive(Debug, Clone, PartialEq)]
pub enum SheetBlock {
    /// A section header (`Verse 1`, `Chorus`).
    Section(String),
    /// A lyric line with chords anchored to its characters.
    Line {
        /// The lyric (may be empty for a chord-only line).
        text: String,
        /// The chords with the character index they sit at, ascending.
        chords: Vec<(usize, ChordSymbol)>,
        /// The bar of the song this line starts at, when it is linked.
        bar: Option<u32>,
    },
    /// A block of tab drawn as text.
    Tab(TabBlockText),
    /// An empty line.
    Blank,
    /// A comment that is shown but is not lyrics.
    Comment(String),
}

impl SheetBlock {
    /// A lyric line without chords.
    pub fn lyric(text: &str) -> Self {
        SheetBlock::Line {
            text: text.to_owned(),
            chords: Vec::new(),
            bar: None,
        }
    }
}

/// A chord sheet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChordSheet {
    /// The title.
    pub title: String,
    /// The artist.
    pub artist: String,
    /// The key the chords are written in.
    pub key: Option<Key>,
    /// The capo fret the chords are shaped for (0 = none).
    pub capo: u8,
    /// The content.
    pub blocks: Vec<SheetBlock>,
    /// Other `name: value` facts (subtitle, tempo, time, ...), in order.
    pub meta: Vec<(String, String)>,
}

/// A parsed sheet and what was ignored or cut.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetParse {
    /// The sheet.
    pub sheet: ChordSheet,
    /// Things the reader did not keep, for the user.
    pub warnings: Vec<String>,
}

fn clean_lines(text: &str) -> Result<(Vec<String>, Vec<String>), SheetError> {
    if text.len() > MAX_SHEET_BYTES {
        return Err(SheetError::TooLarge(text.len()));
    }
    let mut warnings = Vec::new();
    let mut lines = Vec::new();
    let mut cut = 0usize;
    for raw in text.split('\n') {
        if lines.len() >= MAX_LINES {
            return Err(SheetError::TooManyLines);
        }
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let mut line = String::with_capacity(raw.len().min(MAX_LINE_CHARS + 8));
        let mut n = 0usize;
        for c in raw.chars() {
            if n >= MAX_LINE_CHARS {
                cut += 1;
                break;
            }
            match c {
                '\t' => {
                    line.push_str("    ");
                    n += 4;
                }
                c if c.is_control() => {
                    line.push(' ');
                    n += 1;
                }
                c => {
                    line.push(c);
                    n += 1;
                }
            }
        }
        lines.push(line);
    }
    if cut > 0 {
        warnings.push(format!(
            "{cut} lines were longer than {MAX_LINE_CHARS} characters and were cut"
        ));
    }
    if text.ends_with('\n') && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    Ok((lines, warnings))
}

/// Reads a sheet, choosing the inline or the chords-over-lyrics reader by what the text looks
/// like.
pub fn parse_sheet(text: &str) -> Result<SheetParse, SheetError> {
    if looks_inline(text) {
        parse_inline(text)
    } else {
        parse_over_lyrics(text)
    }
}

/// Whether the text uses the inline bracket form: it has a `{directive}` line or a lyric line
/// with a `[chord]` in it.
pub fn looks_inline(text: &str) -> bool {
    for line in text.lines().take(400) {
        let t = line.trim();
        if t.starts_with('{') && t.ends_with('}') && t.contains(|c: char| c.is_ascii_alphabetic()) {
            return true;
        }
        if let Some(open) = t.find('[')
            && let Some(close) = t[open..].find(']')
        {
            let inner = &t[open + 1..open + close];
            let rest = format!("{}{}", &t[..open], &t[open + close + 1..]);
            if ChordSymbol::parse(inner).is_ok() && !rest.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

const SECTION_WORDS: [&str; 20] = [
    "intro",
    "verse",
    "chorus",
    "pre-chorus",
    "prechorus",
    "pre chorus",
    "bridge",
    "outro",
    "interlude",
    "solo",
    "instrumental",
    "refrain",
    "hook",
    "tag",
    "coda",
    "break",
    "ending",
    "turnaround",
    "riff",
    "middle 8",
];

/// A section header, with the brackets and the colon removed.
fn section_name(line: &str) -> Option<String> {
    let t = line.trim();
    if t.is_empty() || t.chars().count() > 40 {
        return None;
    }
    // `[Anything]` alone on a line, with no other bracket inside.
    let inner = t
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .filter(|i| !i.contains('[') && !i.contains(']'))
        .map(str::trim);
    if let Some(i) = inner {
        return (!i.is_empty() && ChordSymbol::parse(i).is_err()).then(|| i.to_owned());
    }
    // A bare header is a known word, optionally numbered, optionally with a colon:
    // `Chorus`, `Verse 2:`.
    let bare = t.strip_suffix(':').unwrap_or(t).trim();
    let lower = bare.to_lowercase();
    SECTION_WORDS
        .iter()
        .any(|w| {
            lower == *w
                || lower.strip_prefix(w).is_some_and(|r| {
                    !r.trim().is_empty() && r.trim().chars().all(|c| c.is_ascii_digit())
                })
        })
        .then(|| bare.to_owned())
}

/// The section name of a header line (`[Verse]`, `Chorus 2:`), for the tab reader.
pub(crate) fn section_name_of(line: &str) -> Option<String> {
    section_name(line)
}

/// Whether a token is a bar, a repeat count or a rest marker that may sit on a chord line.
fn neutral_token(t: &str) -> bool {
    if t.chars()
        .all(|c| matches!(c, '|' | ':' | '/' | '-' | '.' | '*' | '(' | ')' | '%' | '~'))
    {
        return true;
    }
    let u = t.to_ascii_uppercase();
    if matches!(
        u.as_str(),
        "N.C." | "NC" | "N.C" | "(N.C.)" | "REPEAT" | "REP."
    ) {
        return true;
    }
    let digits = t.trim_matches(|c| matches!(c, '(' | ')' | 'x' | 'X'));
    !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && (t.contains('x') || t.contains('X'))
}

/// The chord tokens of a line with their character columns, if the line is a chord line.
fn chord_line(line: &str) -> Option<Vec<(usize, ChordSymbol)>> {
    let mut chords = Vec::new();
    let mut others = 0usize;
    let mut start: Option<usize> = None;
    let mut word = String::new();
    let flush = |col_start: usize,
                 w: &mut String,
                 chords: &mut Vec<(usize, ChordSymbol)>,
                 others: &mut usize| {
        if w.is_empty() {
            return;
        }
        // Brackets around a chord are tolerated: `(Am)`, `[Am]`.
        let core = w.trim_matches(|c| matches!(c, '(' | ')' | '[' | ']' | '|' | ','));
        let lead = w.len() - w.trim_start_matches(['(', '[', '|']).len();
        if let Ok(c) = ChordSymbol::parse(core) {
            chords.push((col_start + lead, c));
        } else if !neutral_token(w) {
            *others += 1;
        }
        w.clear();
    };
    for (col, ch) in line.chars().enumerate() {
        if ch == ' ' {
            if let Some(s) = start.take() {
                flush(s, &mut word, &mut chords, &mut others);
            }
        } else {
            if start.is_none() {
                start = Some(col);
            }
            word.push(ch);
        }
    }
    if let Some(s) = start.take() {
        flush(s, &mut word, &mut chords, &mut others);
    }
    if chords.is_empty() {
        return None;
    }
    let total = chords.len() + others;
    (chords.len() * 100 >= total * 60).then_some(chords)
}

/// A `name: value` directive in braces, name lower-cased.
fn directive(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let inner = t.strip_prefix('{')?.strip_suffix('}')?;
    let (name, value) = match inner.split_once(':') {
        Some((n, v)) => (n.trim(), v.trim()),
        None => match inner.split_once(' ') {
            Some((n, v)) if !n.is_empty() => (n.trim(), v.trim()),
            _ => (inner.trim(), ""),
        },
    };
    if name.is_empty() {
        return None;
    }
    Some((name.to_lowercase(), value.to_owned()))
}

fn apply_meta(sheet: &mut ChordSheet, name: &str, value: &str) -> bool {
    match name {
        "title" | "t" => sheet.title = value.to_owned(),
        "artist" => sheet.artist = value.to_owned(),
        "key" => {
            if let Some(k) = Key::parse(value) {
                sheet.key = Some(k);
            } else {
                sheet.meta.push((name.to_owned(), value.to_owned()));
            }
        }
        "capo" => {
            let digits: String = value.chars().filter(char::is_ascii_digit).collect();
            sheet.capo = digits.parse::<u8>().unwrap_or(0).min(24);
        }
        "subtitle" | "st" | "tempo" | "time" | "duration" | "album" | "composer" | "lyricist"
        | "year" | "copyright" | "meta" => {
            sheet.meta.push((name.to_owned(), value.to_owned()));
        }
        _ => return false,
    }
    true
}

/// A header line of the chords-over-lyrics form: `Key: G`, `Capo: 2`, `Capo 2`, `Title: x`.
fn plain_header(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let lower = t.to_lowercase();
    for name in [
        "title",
        "artist",
        "key",
        "capo",
        "tempo",
        "subtitle",
        "album",
        "time",
        "duration",
        "composer",
        "lyricist",
        "year",
        "copyright",
    ] {
        if let Some(rest) = lower.strip_prefix(name) {
            let rest_t = &t[name.len()..];
            if rest.starts_with(':') {
                return Some((name.to_owned(), rest_t[1..].trim().to_owned()));
            }
            if name == "capo" && rest.trim_start().starts_with(|c: char| c.is_ascii_digit()) {
                return Some((name.to_owned(), rest_t.trim().to_owned()));
            }
        }
    }
    None
}

fn push_line(
    blocks: &mut Vec<SheetBlock>,
    text: String,
    mut chords: Vec<(usize, ChordSymbol)>,
    warnings: &mut Vec<String>,
    cut_lines: &mut usize,
) {
    if chords.len() > MAX_CHORDS_PER_LINE {
        chords.truncate(MAX_CHORDS_PER_LINE);
        *cut_lines += 1;
    }
    let _ = warnings;
    blocks.push(SheetBlock::Line {
        text,
        chords,
        bar: None,
    });
}

/// Reads the chords-over-lyrics form (see the module documentation).
pub fn parse_over_lyrics(text: &str) -> Result<SheetParse, SheetError> {
    let (lines, mut warnings) = clean_lines(text)?;
    let mut sheet = ChordSheet::default();
    let mut cut_lines = 0usize;
    let mut i = 0usize;
    while i < lines.len() {
        let line = &lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            sheet.blocks.push(SheetBlock::Blank);
            i += 1;
            continue;
        }
        if let Some((n, v)) = directive(trimmed)
            && apply_directive(&mut sheet, &n, &v)
        {
            i += 1;
            continue;
        }
        // A run of tab lines.
        let mut j = i;
        while j < lines.len() && crate::ascii::looks_like_tab_line(&lines[j]) {
            j += 1;
        }
        if j - i >= 2 {
            sheet.blocks.push(SheetBlock::Tab(TabBlockText {
                lines: lines[i..j].to_vec(),
            }));
            i = j;
            continue;
        }
        if !sheet
            .blocks
            .iter()
            .any(|b| matches!(b, SheetBlock::Line { .. } | SheetBlock::Section(_)))
            && let Some((n, v)) = plain_header(trimmed)
            && apply_meta(&mut sheet, &n, &v)
        {
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            sheet
                .blocks
                .push(SheetBlock::Comment(rest.trim().to_owned()));
            i += 1;
            continue;
        }
        if let Some(name) = section_name(trimmed)
            && chord_line(line).is_none()
        {
            sheet.blocks.push(SheetBlock::Section(name));
            i += 1;
            continue;
        }
        if let Some(chords) = chord_line(line) {
            let next = lines.get(i + 1);
            let lyric_below = next.is_some_and(|n| {
                let nt = n.trim();
                !nt.is_empty()
                    && chord_line(n).is_none()
                    && !nt.split_whitespace().all(neutral_token)
                    && section_name(nt).is_none()
                    && !crate::ascii::looks_like_tab_line(n)
                    && !nt.starts_with('#')
                    && !(nt.starts_with('{') && nt.ends_with('}'))
            });
            if lyric_below {
                let lyric = next.map(|n| n.trim_end().to_owned()).unwrap_or_default();
                push_line(
                    &mut sheet.blocks,
                    lyric,
                    chords,
                    &mut warnings,
                    &mut cut_lines,
                );
                i += 2;
            } else {
                push_line(
                    &mut sheet.blocks,
                    String::new(),
                    chords,
                    &mut warnings,
                    &mut cut_lines,
                );
                i += 1;
            }
            continue;
        }
        sheet.blocks.push(SheetBlock::lyric(line.trim_end()));
        i += 1;
    }
    if cut_lines > 0 {
        warnings.push(format!(
            "{cut_lines} lines had more than {MAX_CHORDS_PER_LINE} chords; the rest were dropped"
        ));
    }
    trim_blank_edges(&mut sheet.blocks);
    Ok(SheetParse { sheet, warnings })
}

fn trim_blank_edges(blocks: &mut Vec<SheetBlock>) {
    while matches!(blocks.last(), Some(SheetBlock::Blank)) {
        blocks.pop();
    }
    let lead = blocks
        .iter()
        .take_while(|b| matches!(b, SheetBlock::Blank))
        .count();
    blocks.drain(..lead);
}

fn apply_directive(sheet: &mut ChordSheet, name: &str, value: &str) -> bool {
    if apply_meta(sheet, name, value) {
        return true;
    }
    match name {
        "comment" | "c" | "comment_italic" | "ci" | "comment_box" | "cb" => {
            sheet.blocks.push(SheetBlock::Comment(value.to_owned()));
            true
        }
        "start_of_chorus" | "soc" => {
            sheet.blocks.push(SheetBlock::Section(if value.is_empty() {
                "Chorus".to_owned()
            } else {
                value.to_owned()
            }));
            true
        }
        "start_of_verse" | "sov" => {
            sheet.blocks.push(SheetBlock::Section(if value.is_empty() {
                "Verse".to_owned()
            } else {
                value.to_owned()
            }));
            true
        }
        "start_of_bridge" | "sob" => {
            sheet.blocks.push(SheetBlock::Section(if value.is_empty() {
                "Bridge".to_owned()
            } else {
                value.to_owned()
            }));
            true
        }
        "end_of_chorus" | "eoc" | "end_of_verse" | "eov" | "end_of_bridge" | "eob" | "define"
        | "chord" | "new_page" | "np" | "new_physical_page" | "npp" | "column_break" | "cb2"
        | "columns" | "col" | "textsize" | "textfont" | "chordsize" | "chordfont" => true,
        _ => false,
    }
}

/// Reads the inline bracket form (see the module documentation).
pub fn parse_inline(text: &str) -> Result<SheetParse, SheetError> {
    let (lines, mut warnings) = clean_lines(text)?;
    let mut sheet = ChordSheet::default();
    let mut cut_lines = 0usize;
    let mut unknown = 0usize;
    let mut tab: Option<TabBlockText> = None;
    for line in &lines {
        let trimmed = line.trim();
        if let Some(block) = tab.as_mut() {
            if let Some((n, _)) = directive(trimmed)
                && (n == "end_of_tab" || n == "eot")
            {
                if let Some(b) = tab.take() {
                    sheet.blocks.push(SheetBlock::Tab(b));
                }
            } else {
                block.lines.push(line.clone());
            }
            continue;
        }
        if trimmed.is_empty() {
            sheet.blocks.push(SheetBlock::Blank);
            continue;
        }
        if let Some((n, v)) = directive(trimmed) {
            if n == "start_of_tab" || n == "sot" {
                tab = Some(TabBlockText::default());
            } else if !apply_directive(&mut sheet, &n, &v) {
                unknown += 1;
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            sheet
                .blocks
                .push(SheetBlock::Comment(rest.trim().to_owned()));
            continue;
        }
        if let Some(name) = section_name(trimmed) {
            sheet.blocks.push(SheetBlock::Section(name));
            continue;
        }
        let (text, chords) = split_inline(line);
        push_line(
            &mut sheet.blocks,
            text,
            chords,
            &mut warnings,
            &mut cut_lines,
        );
    }
    if let Some(b) = tab.take() {
        sheet.blocks.push(SheetBlock::Tab(b));
    }
    if unknown > 0 {
        warnings.push(format!("{unknown} unknown directives were ignored"));
    }
    if cut_lines > 0 {
        warnings.push(format!(
            "{cut_lines} lines had more than {MAX_CHORDS_PER_LINE} chords; the rest were dropped"
        ));
    }
    trim_blank_edges(&mut sheet.blocks);
    Ok(SheetParse { sheet, warnings })
}

/// Splits `[Am]Hel[G]lo` into the text and the chords with their character indices. A bracket
/// that is not a chord stays in the text.
fn split_inline(line: &str) -> (String, Vec<(usize, ChordSymbol)>) {
    let chars: Vec<char> = line.chars().collect();
    let mut text = String::new();
    let mut count = 0usize;
    let mut chords = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '['
            && let Some(len) = chars[i + 1..].iter().take(40).position(|c| *c == ']')
        {
            let inner: String = chars[i + 1..i + 1 + len].iter().collect();
            if let Ok(c) = ChordSymbol::parse(&inner) {
                chords.push((count, c));
                i += len + 2;
                continue;
            }
        }
        text.push(chars[i]);
        count += 1;
        i += 1;
    }
    (text.trim_end().to_owned(), chords)
}

impl ChordSheet {
    /// The text of every lyric line, for searching or printing without chords.
    pub fn lyrics(&self) -> String {
        let mut out = String::new();
        for b in &self.blocks {
            if let SheetBlock::Line { text, .. } = b
                && !text.trim().is_empty()
            {
                out.push_str(text);
                out.push('\n');
            }
        }
        out
    }

    /// The distinct chords of the sheet in order of first appearance.
    pub fn chords_used(&self) -> Vec<ChordSymbol> {
        let mut out: Vec<ChordSymbol> = Vec::new();
        for b in &self.blocks {
            if let SheetBlock::Line { chords, .. } = b {
                for (_, c) in chords {
                    if !out.contains(c) {
                        out.push(c.clone());
                    }
                }
            }
        }
        out
    }

    /// Moves every chord (and the key) by `semitones`. With no `spelling` the target key decides
    /// when the sheet has one, else the sheet's own habit (flats when it writes more flats than
    /// sharps).
    pub fn transpose(&mut self, semitones: i32, spelling: Option<Spelling>) {
        let sp = match spelling {
            Some(s) => s,
            None => match self.key {
                Some(k) => Spelling::Key(transpose_key(k, semitones)),
                None => {
                    if self.writes_flats() {
                        Spelling::Flats
                    } else {
                        Spelling::Sharps
                    }
                }
            },
        };
        if let Some(k) = self.key {
            self.key = Some(match sp {
                Spelling::Key(nk) => nk,
                _ => transpose_key(k, semitones),
            });
        }
        for b in &mut self.blocks {
            if let SheetBlock::Line { chords, .. } = b {
                for (_, c) in chords.iter_mut() {
                    *c = transpose_chord(c, semitones, sp);
                }
            }
        }
    }

    fn writes_flats(&self) -> bool {
        let mut flats = 0i32;
        let mut sharps = 0i32;
        for c in self.chords_used() {
            for n in std::iter::once(c.root).chain(c.bass) {
                match n.accidental.signum() {
                    -1 => flats += 1,
                    1 => sharps += 1,
                    _ => {}
                }
            }
        }
        flats > sharps
    }

    /// The sheet as the shapes to play with a capo at `capo`: chords `capo` semitones lower,
    /// `capo` set. A capo already set is replaced.
    pub fn with_capo(&self, capo: u8) -> ChordSheet {
        let mut s = self.clone();
        // Back to concert pitch, then down to the new shapes.
        let delta = i32::from(self.capo) - i32::from(capo);
        if delta != 0 {
            s.transpose(delta, None);
        }
        s.capo = capo.min(24);
        s
    }

    /// Writes the chords-over-lyrics form.
    pub fn to_over_lyrics(&self) -> String {
        let mut out = String::new();
        for (name, v) in self.header() {
            out.push_str(&format!("{name}: {v}\n"));
        }
        if !out.is_empty() {
            out.push('\n');
        }
        for b in &self.blocks {
            match b {
                SheetBlock::Section(n) => out.push_str(&format!("[{n}]\n")),
                SheetBlock::Blank => out.push('\n'),
                SheetBlock::Comment(c) => out.push_str(&format!("# {c}\n")),
                SheetBlock::Tab(t) => {
                    for l in &t.lines {
                        out.push_str(l);
                        out.push('\n');
                    }
                }
                SheetBlock::Line { text, chords, .. } => {
                    if !chords.is_empty() {
                        out.push_str(&chord_row(chords));
                        out.push('\n');
                    }
                    if !text.is_empty() || chords.is_empty() {
                        out.push_str(text);
                        out.push('\n');
                    }
                }
            }
        }
        out
    }

    /// Writes the inline bracket form with directives.
    pub fn to_inline(&self) -> String {
        let mut out = String::new();
        for (name, v) in self.header() {
            out.push_str(&format!("{{{name}: {v}}}\n"));
        }
        if !out.is_empty() {
            out.push('\n');
        }
        for b in &self.blocks {
            match b {
                SheetBlock::Section(n) => out.push_str(&format!("[{n}]\n")),
                SheetBlock::Blank => out.push('\n'),
                SheetBlock::Comment(c) => out.push_str(&format!("{{comment: {c}}}\n")),
                SheetBlock::Tab(t) => {
                    out.push_str("{start_of_tab}\n");
                    for l in &t.lines {
                        out.push_str(l);
                        out.push('\n');
                    }
                    out.push_str("{end_of_tab}\n");
                }
                SheetBlock::Line { text, chords, .. } => {
                    let chars: Vec<char> = text.chars().collect();
                    let mut sorted: Vec<&(usize, ChordSymbol)> = chords.iter().collect();
                    sorted.sort_by_key(|c| c.0);
                    let mut pos = 0usize;
                    for (idx, c) in sorted {
                        let idx = (*idx).min(chars.len() + MAX_LINE_CHARS);
                        while pos < idx {
                            out.push(chars.get(pos).copied().unwrap_or(' '));
                            pos += 1;
                        }
                        out.push('[');
                        out.push_str(&c.name());
                        out.push(']');
                    }
                    while pos < chars.len() {
                        out.push(chars[pos]);
                        pos += 1;
                    }
                    out.push('\n');
                }
            }
        }
        out
    }

    fn header(&self) -> Vec<(String, String)> {
        let mut h = Vec::new();
        if !self.title.is_empty() {
            h.push(("title".to_owned(), self.title.clone()));
        }
        if !self.artist.is_empty() {
            h.push(("artist".to_owned(), self.artist.clone()));
        }
        if let Some(k) = self.key {
            h.push(("key".to_owned(), k.name()));
        }
        if self.capo > 0 {
            h.push(("capo".to_owned(), self.capo.to_string()));
        }
        for (n, v) in &self.meta {
            h.push((n.clone(), v.clone()));
        }
        h
    }
}

/// The chord line above a lyric: each chord at its index, at least one space apart.
fn chord_row(chords: &[(usize, ChordSymbol)]) -> String {
    let mut sorted: Vec<&(usize, ChordSymbol)> = chords.iter().collect();
    sorted.sort_by_key(|c| c.0);
    let mut row = String::new();
    let mut cursor = 0usize;
    for (idx, c) in sorted {
        let col = (*idx).max(cursor).min(MAX_LINE_CHARS * 2);
        while cursor < col {
            row.push(' ');
            cursor += 1;
        }
        let name = c.name();
        cursor += name.chars().count();
        row.push_str(&name);
        row.push(' ');
        cursor += 1;
    }
    row.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chords_of(sheet: &ChordSheet, line: usize) -> Vec<(usize, String)> {
        let lines: Vec<&SheetBlock> = sheet
            .blocks
            .iter()
            .filter(|b| matches!(b, SheetBlock::Line { .. }))
            .collect();
        match lines[line] {
            SheetBlock::Line { chords, .. } => chords.iter().map(|(i, c)| (*i, c.name())).collect(),
            _ => unreachable!(),
        }
    }

    const OVER: &str = "Title: Paper Boats
Artist: Imaginary Band
Key: G
Capo: 2

[Verse 1]
G            D
Sailing down the little stream
Em           C
Paper boats go by
[Chorus]
G   D   Em  C
(x2)
        G        D
Row, row, row along
";

    #[test]
    fn chords_over_lyrics_anchor_to_the_character_below() {
        let p = parse_over_lyrics(OVER).unwrap();
        let s = &p.sheet;
        assert_eq!(s.title, "Paper Boats");
        assert_eq!(s.artist, "Imaginary Band");
        assert_eq!(s.key.map(|k| k.name()), Some("G".to_owned()));
        assert_eq!(s.capo, 2);
        assert!(matches!(&s.blocks[0], SheetBlock::Section(n) if n == "Verse 1"));
        assert_eq!(
            chords_of(s, 0),
            vec![(0, "G".to_owned()), (13, "D".to_owned())]
        );
        match &s.blocks[1] {
            SheetBlock::Line { text, .. } => assert_eq!(text, "Sailing down the little stream"),
            b => panic!("{b:?}"),
        }
        // The chord at column 13 sits over the `t` of "the".
        assert_eq!("Sailing down the little stream".chars().nth(13), Some('t'));
        // The chord-only line keeps empty text.
        match &s.blocks[4] {
            SheetBlock::Line { text, chords, .. } => {
                assert!(text.is_empty());
                assert_eq!(chords.len(), 4);
            }
            b => panic!("{b:?}"),
        }
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn inline_form_with_directives() {
        let src = "{title: Little Lanterns}
{artist: Nobody}
{key: Am}
{capo: 3}
{comment: slowly}
{start_of_chorus}
[Am]Light the [F]lanterns [C]one by [G]one
{end_of_chorus}
[Bridge]
[Am] [G] [F]
{start_of_tab}
e|---0---|
B|---1---|
{end_of_tab}
";
        let p = parse_sheet(src).unwrap();
        let s = &p.sheet;
        assert_eq!(s.title, "Little Lanterns");
        assert_eq!(s.key.map(|k| k.name()), Some("Am".to_owned()));
        assert_eq!(s.capo, 3);
        assert!(matches!(&s.blocks[0], SheetBlock::Comment(c) if c == "slowly"));
        assert!(matches!(&s.blocks[1], SheetBlock::Section(n) if n == "Chorus"));
        let c = chords_of(s, 0);
        assert_eq!(c.len(), 4);
        assert_eq!(c[0], (0, "Am".to_owned()));
        assert_eq!(c[1], (10, "F".to_owned()));
        assert!(matches!(&s.blocks[3], SheetBlock::Section(n) if n == "Bridge"));
        assert_eq!(chords_of(s, 1).len(), 3);
        assert!(
            s.blocks
                .iter()
                .any(|b| matches!(b, SheetBlock::Tab(t) if t.lines.len() == 2))
        );
    }

    #[test]
    fn the_two_forms_are_told_apart() {
        assert!(looks_inline("[C]hello [G]world"));
        assert!(looks_inline("{title: x}"));
        assert!(!looks_inline(OVER));
        // A section header alone is not enough for the inline reader.
        assert!(!looks_inline("[Verse]\nC G\nhello world"));
    }

    #[test]
    fn ordinary_lyrics_are_not_chord_lines() {
        let p = parse_over_lyrics("A tree grew by the river\nBe good to each other\nAm I a fool\n")
            .unwrap();
        assert!(
            p.sheet
                .blocks
                .iter()
                .all(|b| matches!(b, SheetBlock::Line { chords, .. } if chords.is_empty()))
        );
    }

    #[test]
    fn section_header_forms() {
        let p = parse_over_lyrics("Intro\nVerse 2:\n[Pre-Chorus]\n[Solo - guitar]\n").unwrap();
        let names: Vec<_> = p
            .sheet
            .blocks
            .iter()
            .map(|b| match b {
                SheetBlock::Section(n) => n.clone(),
                b => format!("{b:?}"),
            })
            .collect();
        assert_eq!(
            names,
            vec!["Intro", "Verse 2", "Pre-Chorus", "Solo - guitar"]
        );
    }

    #[test]
    fn writers_round_trip() {
        let p = parse_over_lyrics(OVER).unwrap();
        let plain = p.sheet.to_over_lyrics();
        let again = parse_over_lyrics(&plain).unwrap().sheet;
        assert_eq!(again, p.sheet);
        let inline = p.sheet.to_inline();
        let again = parse_inline(&inline).unwrap().sheet;
        assert_eq!(again, p.sheet);
        // And the inline text reads the same through the auto detector.
        assert_eq!(parse_sheet(&inline).unwrap().sheet, p.sheet);
    }

    #[test]
    fn transpose_with_a_key_spells_for_the_target() {
        let mut s = parse_over_lyrics(OVER).unwrap().sheet;
        s.transpose(3, None);
        assert_eq!(s.key.map(|k| k.name()), Some("Bb".to_owned()));
        let used: Vec<String> = s.chords_used().iter().map(|c| c.name()).collect();
        assert_eq!(used, vec!["Bb", "F", "Gm", "Eb"]);
        s.transpose(-3, None);
        let used: Vec<String> = s.chords_used().iter().map(|c| c.name()).collect();
        assert_eq!(used, vec!["G", "D", "Em", "C"]);
    }

    #[test]
    fn capo_shapes_view() {
        let s = parse_inline("{key: Bb}\n[Bb]one [Eb]two [F]three [Gm]four\n")
            .unwrap()
            .sheet;
        let shapes = s.with_capo(3);
        assert_eq!(shapes.capo, 3);
        let used: Vec<String> = shapes.chords_used().iter().map(|c| c.name()).collect();
        assert_eq!(used, vec!["G", "C", "D", "Em"]);
        assert_eq!(shapes.key.map(|k| k.name()), Some("G".to_owned()));
        // Changing the capo again goes through concert pitch.
        let back = shapes.with_capo(0);
        let used: Vec<String> = back.chords_used().iter().map(|c| c.name()).collect();
        assert_eq!(used, vec!["Bb", "Eb", "F", "Gm"]);
    }

    #[test]
    fn transpose_without_a_key_keeps_the_habit() {
        let mut s = parse_inline("[Bb]a [Eb]b [F]c\n").unwrap().sheet;
        s.transpose(1, None);
        let used: Vec<String> = s.chords_used().iter().map(|c| c.name()).collect();
        assert_eq!(used, vec!["B", "E", "Gb"]);
        let mut s = parse_inline("[F#]a [B]b [C#]c\n").unwrap().sheet;
        s.transpose(-2, None);
        let used: Vec<String> = s.chords_used().iter().map(|c| c.name()).collect();
        assert_eq!(used, vec!["E", "A", "B"]);
    }

    #[test]
    fn tab_blocks_survive() {
        let src = "Verse\nC\nsome words\ne|--0--|\nB|--1--|\nG|--0--|\n";
        let p = parse_over_lyrics(src).unwrap();
        assert!(
            p.sheet
                .blocks
                .iter()
                .any(|b| matches!(b, SheetBlock::Tab(t) if t.lines.len() == 3))
        );
    }

    #[test]
    fn hostile_input_never_panics() {
        for text in [
            "",
            "\n\n\n",
            "[",
            "[]",
            "[[[[[[",
            "{",
            "{}",
            "{:}",
            "}{",
            "[C",
            "C C C C C C C C",
            "\u{0}\u{1}\u{2}",
            "[C]\u{202e}text",
            "🎸[Am]🎸",
            "{start_of_tab}",
            "x2 x3 | | |",
        ] {
            let _ = parse_sheet(text);
            let _ = parse_over_lyrics(text);
            let _ = parse_inline(text);
        }
        let huge_line = "[C]a".repeat(5000);
        let p = parse_inline(&huge_line).unwrap();
        assert!(!p.warnings.is_empty());
        assert!(chords_of(&p.sheet, 0).len() <= MAX_CHORDS_PER_LINE);
        let many = "line\n".repeat(MAX_LINES + 5);
        assert_eq!(parse_sheet(&many), Err(SheetError::TooManyLines));
        let big = "x".repeat(MAX_SHEET_BYTES + 1);
        assert!(matches!(parse_sheet(&big), Err(SheetError::TooLarge(_))));
    }

    #[test]
    fn chords_used_are_distinct_in_order() {
        let s = parse_inline("[G]a [D]b [G]c [Em]d [D]e\n").unwrap().sheet;
        let used: Vec<String> = s.chords_used().iter().map(|c| c.name()).collect();
        assert_eq!(used, vec!["G", "D", "Em"]);
        assert_eq!(s.lyrics().trim(), "a b c d e");
    }

    #[test]
    fn chord_row_never_overlaps() {
        let chords = vec![
            (0usize, ChordSymbol::parse("Cmaj7").unwrap()),
            (2, ChordSymbol::parse("G7#9").unwrap()),
        ];
        let row = chord_row(&chords);
        assert_eq!(row, "Cmaj7 G7#9");
    }
}
