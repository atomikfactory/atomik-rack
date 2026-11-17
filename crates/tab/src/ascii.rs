//! Plain-text (ASCII) tab: reading and writing.
//!
//! # Reading
//!
//! A block of consecutive string lines (`e|---3---|`, any labels and any number of strings, up to
//! 12) is one system. The tuning comes from the labels when they are note names (`E A D G B e`
//! gives a standard guitar; a bass, ukulele or banjo is found the same way when the labels match a
//! catalogue tuning), else from the line count (4 and 5 lines are a bass, 6 to 8 a guitar). The
//! columns between the bar lines are the time axis: **every bar is an even grid of its columns**,
//! so a bar of 16 columns reads as sixteenth notes in 4/4 and a bar of 8 as eighths. Real rhythm
//! cannot be read from spacing, this is an approximation (documented in the user guide); the meter
//! and tempo come from [`AsciiOptions`].
//!
//! Marks read inside a string line:
//!
//! | text | meaning |
//! | --- | --- |
//! | `12` | a fret (one or two digits) |
//! | `x` | a dead note |
//! | `5h7` / `7p5` | hammer-on / pull-off (the flag goes on the second note) |
//! | `5/7` `7\5` `5s7` | a legato slide from the first note |
//! | `/7` `\7` | a slide into the note from below / above |
//! | `7/` `7\` | a slide out of the note, up / down |
//! | `7b9` `7b9r7` `7b` | a bend to the fret (a plain `b` is a full step); with `r`, released |
//! | `7~` `7~~` | vibrato, wide |
//! | `(5)` | a ghost note |
//! | `<12>` `[12]` | a natural / artificial harmonic |
//! | `t12` | a tapped note |
//! | `PM---` above the lines | palm mute over the columns it spans |
//!
//! A repeat count after a system (`x2`, `2x`, `(x3)`) repeats its bars, expanded. A short line
//! above a system that is a section name (`[Verse]`, `Chorus`) becomes a marker. The first line of
//! the text becomes the title when a blank line follows it. Anything else is ignored. The input
//! is capped ([`MAX_ASCII_BYTES`], [`MAX_ASCII_LINES`]) and nothing panics on garbage.
//!
//! # Writing
//!
//! [`write_ascii`] draws every non-percussion track with a fixed column grid per bar: the grid
//! unit is the greatest common divisor of the note offsets of the bar (at most 64 slots per bar;
//! beyond that onsets are rounded to the grid), every slot is as wide as the longest note text of
//! the bar plus two, systems wrap at a width and a legend lists the marks used. Reading the text
//! back with the same meter gives the same notes (tick, string, fret) and techniques, except
//! ties (not drawn), pre-bends and the note lengths (not drawn), and a hammer-on or pull-off on
//! the first slot of a bar (its marker has no room).

use crate::instrument::{Tuning, catalogue, find_instrument};
use crate::score::{
    Bend, BendKind, Harmonic, HarmonicKind, SlideKind, TabNote, TabScore, TabTrack, TechFlags,
    Techniques, VibratoKind, bar_ticks, limits,
};

/// The most bytes of ASCII tab that is read.
pub const MAX_ASCII_BYTES: usize = 4 * 1024 * 1024;
/// The most lines of ASCII tab that is read.
pub const MAX_ASCII_LINES: usize = 60_000;
/// The most characters read of one line.
pub const MAX_ASCII_LINE: usize = 4_000;
/// The most systems read.
pub const MAX_SYSTEMS: usize = 6_000;

/// Why ASCII tab could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AsciiError {
    /// The text is bigger than the limit.
    #[error("the text is too large ({0} bytes, at most {MAX_ASCII_BYTES})")]
    TooLarge(usize),
    /// The text has too many lines.
    #[error("the text has too many lines (at most {MAX_ASCII_LINES})")]
    TooManyLines,
    /// No string lines were found.
    #[error("no tab lines (such as e|---3---|) were found in the text")]
    NoTab,
}

/// What the reader assumes about the music the text cannot say.
#[derive(Debug, Clone, PartialEq)]
pub struct AsciiOptions {
    /// Beats per bar.
    pub numerator: u8,
    /// The beat note (4 = quarter).
    pub denominator: u8,
    /// The tempo.
    pub bpm: f64,
    /// The tuning to use instead of reading the labels (when its string count matches).
    pub tuning: Option<Tuning>,
    /// The velocity of every note.
    pub velocity: u8,
}

impl Default for AsciiOptions {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
            bpm: 100.0,
            tuning: None,
            velocity: 90,
        }
    }
}

/// A tab read from text and what was not understood.
#[derive(Debug, Clone, PartialEq)]
pub struct AsciiParse {
    /// The score (one track per distinct string count).
    pub score: TabScore,
    /// Things the reader ignored, for the user.
    pub warnings: Vec<String>,
}

/// One string line, split into its label and its body.
#[derive(Debug, Clone, PartialEq)]
struct StringLine {
    label: Option<String>,
    /// Index of the first body character in the line.
    start: usize,
    body: Vec<char>,
    trailing: String,
}

const BODY_CHARS: &str = "-|0123456789hpbrxX/\\~()<>[]*^.sStTvVPMm=:'\"_";

fn split_line(line: &str) -> Option<StringLine> {
    let chars: Vec<char> = line.chars().take(MAX_ASCII_LINE).collect();
    let mut i = 0usize;
    while chars.get(i) == Some(&' ') {
        i += 1;
    }
    let mut label = None;
    let first = *chars.get(i)?;
    if matches!(first, 'A'..='G' | 'a'..='g') {
        let mut j = i + 1;
        match chars.get(j) {
            Some('#') => j += 1,
            Some('b') if matches!(chars.get(j + 1), Some('|') | Some(':')) => j += 1,
            _ => {}
        }
        label = Some(chars[i..j].iter().collect::<String>());
        while chars.get(j) == Some(&' ') {
            j += 1;
        }
        if chars.get(j) == Some(&':') && matches!(chars.get(j + 1), Some('|') | Some('-')) {
            j += 1;
        }
        i = j;
    }
    let start = i;
    if !matches!(chars.get(start), Some('|') | Some('-')) {
        return None;
    }
    let mut end = start;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    let body: Vec<char> = chars[start..end].to_vec();
    let trailing: String = chars[end..].iter().collect::<String>().trim().to_owned();
    if body.len() < 5 {
        return None;
    }
    let dashes = body.iter().filter(|c| **c == '-').count();
    let bad = body.iter().filter(|c| !BODY_CHARS.contains(**c)).count();
    let anchored = body.iter().any(|c| *c == '|' || c.is_ascii_digit());
    if dashes < 3 || bad * 10 > body.len() || !anchored {
        return None;
    }
    Some(StringLine {
        label,
        start,
        body,
        trailing,
    })
}

/// Whether a line looks like a string line of an ASCII tab (used by the chord sheet reader to
/// find tab blocks).
pub(crate) fn looks_like_tab_line(line: &str) -> bool {
    split_line(line).is_some()
}

/// A repeat count in the text after a system: `x2`, `2x`, `(x3)`, `×4`.
fn repeat_count(text: &str) -> Option<u32> {
    let t = text.trim().trim_matches(|c| c == '(' || c == ')');
    let digits = t
        .trim_start_matches(['x', 'X', '\u{d7}'])
        .trim_end_matches(['x', 'X', '\u{d7}']);
    if digits.len() == t.len() || digits.is_empty() || digits.len() > 2 {
        return None;
    }
    digits.parse::<u32>().ok().filter(|n| (2..=16).contains(n))
}

#[derive(Debug, Clone, Default)]
struct RawNote {
    string: usize,
    col: usize,
    fret: u8,
    flags: TechFlags,
    slide: Option<SlideKind>,
    bend: Option<Bend>,
    vibrato: Option<VibratoKind>,
    harmonic: Option<HarmonicKind>,
}

#[derive(Default)]
struct LineState {
    pending_flag: Option<TechFlags>,
    /// A slide waiting for its target: the index of the origin note in the output and the
    /// direction character.
    pending_slide: Option<(usize, char)>,
    /// A slide into the next note and the column of its mark.
    pending_in: Option<(SlideKind, usize)>,
}

fn digits_at(body: &[char], mut c: usize, end: usize) -> Option<(u32, usize)> {
    let mut v = 0u32;
    let mut n = 0;
    while c < end && n < 2 {
        match body.get(c).and_then(|ch| ch.to_digit(10)) {
            Some(d) => {
                v = v * 10 + d;
                c += 1;
                n += 1;
            }
            None => break,
        }
    }
    (n > 0).then_some((v, c))
}

/// Reads the notes of one string line between columns `a` and `b` (exclusive).
fn scan_line(
    body: &[char],
    a: usize,
    b: usize,
    string: usize,
    st: &mut LineState,
    out: &mut Vec<RawNote>,
    warnings: &mut Vec<String>,
) {
    let mut c = a;
    while c < b {
        let ch = body[c];
        let col = c;
        let mut note: Option<RawNote> = None;
        match ch {
            '0'..='9' | 't' | 'T' | '(' | '<' | '[' => {
                let (open, p) = match ch {
                    '0'..='9' => (None, c),
                    _ => (Some(ch), c + 1),
                };
                let parsed = digits_at(body, p, b).and_then(|(v, np)| {
                    let close = match open {
                        Some('(') => Some(')'),
                        Some('<') => Some('>'),
                        Some('[') => Some(']'),
                        _ => None,
                    };
                    match close {
                        Some(cl) if np < b && body.get(np) == Some(&cl) => Some((v, np + 1)),
                        Some(_) => None,
                        None => Some((v, np)),
                    }
                });
                let Some((v, np)) = parsed else {
                    c += 1;
                    continue;
                };
                let mut n = RawNote {
                    string,
                    col,
                    fret: u8::try_from(v).unwrap_or(u8::MAX),
                    ..RawNote::default()
                };
                match open {
                    Some('(') => n.flags.insert(TechFlags::GHOST),
                    Some('<') => n.harmonic = Some(HarmonicKind::Natural),
                    Some('[') => n.harmonic = Some(HarmonicKind::Artificial),
                    Some('t') | Some('T') => n.flags.insert(TechFlags::TAP),
                    _ => {}
                }
                c = np;
                note = Some(n);
            }
            'x' | 'X' => {
                let mut n = RawNote {
                    string,
                    col,
                    ..RawNote::default()
                };
                n.flags.insert(TechFlags::DEAD);
                note = Some(n);
                c += 1;
            }
            '/' | '\\' => {
                let prev_digit = c > a && body[c - 1].is_ascii_digit();
                let next_digit = c + 1 < b && body[c + 1].is_ascii_digit();
                if !prev_digit && next_digit {
                    // A mark glued to the next digit slides into that note; a slide still
                    // waiting for a target leaves its own note instead.
                    if let Some((origin, d)) = st.pending_slide.take()
                        && let Some(o) = out.get_mut(origin)
                    {
                        o.slide = Some(if d == '\\' {
                            SlideKind::OutDown
                        } else {
                            SlideKind::OutUp
                        });
                    }
                    st.pending_in = Some((
                        if ch == '/' {
                            SlideKind::InFromBelow
                        } else {
                            SlideKind::InFromAbove
                        },
                        c,
                    ));
                } else if let Some(last) = out.iter().rposition(|n| n.string == string) {
                    st.pending_slide = Some((last, ch));
                }
                c += 1;
            }
            'h' | 'p' => {
                if out.iter().any(|n| n.string == string) {
                    st.pending_flag = Some(if ch == 'h' {
                        TechFlags::HAMMER_ON
                    } else {
                        TechFlags::PULL_OFF
                    });
                }
                c += 1;
            }
            _ => c += 1,
        }
        let Some(mut n) = note else { continue };
        if n.fret > 36 {
            warnings.push(format!(
                "fret {} is beyond any neck and was ignored",
                n.fret
            ));
            continue;
        }
        if let Some((kind, mark_col)) = st.pending_in.take() {
            n.slide = Some(kind);
            n.col = mark_col;
        }
        if let Some(flag) = st.pending_flag.take() {
            n.flags.insert(flag);
        }
        if let Some((origin, _)) = st.pending_slide.take()
            && let Some(o) = out.get_mut(origin)
        {
            o.slide = Some(SlideKind::Legato);
        }
        // Marks that follow the digits and belong to this note.
        loop {
            if c >= b {
                break;
            }
            match body[c] {
                'b' => {
                    c += 1;
                    let cents = match digits_at(body, c, b) {
                        Some((t, np)) => {
                            c = np;
                            let diff = i32::try_from(t).unwrap_or(0) - i32::from(n.fret);
                            if diff > 0 { diff * 100 } else { 100 }
                        }
                        None => 200,
                    };
                    if c < b && body[c] == 'r' {
                        c += 1;
                        if let Some((_, np)) = digits_at(body, c, b) {
                            c = np;
                        }
                        n.bend = Some(Bend::bend_release(cents));
                    } else {
                        n.bend = Some(Bend::up(cents));
                    }
                }
                '~' => {
                    let mut k = 0;
                    while c < b && body[c] == '~' {
                        k += 1;
                        c += 1;
                    }
                    n.vibrato = Some(if k >= 2 {
                        VibratoKind::Wide
                    } else {
                        VibratoKind::Normal
                    });
                }
                m @ ('h' | 'p' | '/' | '\\' | 's' | 'S') => {
                    c += 1;
                    match m {
                        'h' => st.pending_flag = Some(TechFlags::HAMMER_ON),
                        'p' => st.pending_flag = Some(TechFlags::PULL_OFF),
                        _ => {
                            st.pending_slide = Some((out.len(), if m == '\\' { '\\' } else { '/' }))
                        }
                    }
                    break;
                }
                _ => break,
            }
        }
        out.push(n);
    }
    // A slide that never found its target leaves the note.
    if let Some((origin, d)) = st.pending_slide.take()
        && let Some(o) = out.get_mut(origin)
    {
        o.slide = Some(if d == '\\' {
            SlideKind::OutDown
        } else {
            SlideKind::OutUp
        });
    }
}

/// The body columns an annotation line marks as palm muted: from the `PM` to the end of its dashes.
fn palm_mute_spans(annotation: &str, body_start: usize) -> Vec<(usize, usize)> {
    let chars: Vec<char> = annotation.chars().take(MAX_ASCII_LINE).collect();
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let is_pm = (chars[i] == 'P' || chars[i] == 'p')
            && matches!(chars.get(i + 1), Some('M') | Some('m'));
        let is_pdm = chars[i] == 'P'
            && chars.get(i + 1) == Some(&'.')
            && matches!(chars.get(i + 2), Some('M'));
        if is_pm || is_pdm {
            let mut end = i + if is_pdm { 3 } else { 2 };
            if chars.get(end) == Some(&'.') {
                end += 1;
            }
            while end < chars.len() && matches!(chars[end], '-' | '.' | '_' | '~' | '=') {
                end += 1;
            }
            spans.push((i.saturating_sub(body_start), end.saturating_sub(body_start)));
            i = end;
        } else {
            i += 1;
        }
    }
    spans
}

/// Guesses the tuning from labels and line count.
fn tuning_for(labels: &[Option<String>], n: usize, opts: &AsciiOptions) -> (Tuning, String) {
    if let Some(t) = &opts.tuning
        && t.len() == n
    {
        return (t.clone(), find_id(t));
    }
    let pcs: Vec<u8> = labels
        .iter()
        .filter_map(|l| l.as_ref().and_then(|l| note_pc(l)))
        .collect();
    if pcs.len() == n
        && let Some(t) = Tuning::from_pitch_classes(&pcs)
    {
        let id = find_id(&t);
        return (t, id);
    }
    let id = match n {
        4 => "bass4",
        5 => "bass5",
        6 => "guitar6",
        7 => "guitar7",
        8 => "guitar8",
        _ => "",
    };
    if let Some(inst) = find_instrument(id) {
        return (inst.default_tuning().tuning.clone(), id.to_owned());
    }
    let opens: Vec<u8> = (0..n)
        .map(|i| {
            64u8.saturating_sub(u8::try_from(5 * i).unwrap_or(60))
                .max(12)
        })
        .collect();
    (Tuning::from_opens(&opens, 24), String::new())
}

fn find_id(t: &Tuning) -> String {
    for inst in catalogue() {
        if inst.tunings.iter().any(|nt| nt.tuning.opens() == t.opens()) {
            return inst.id.to_owned();
        }
    }
    String::new()
}

fn note_pc(label: &str) -> Option<u8> {
    let mut ch = label.chars();
    let base: i32 = match ch.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let shift = match ch.next() {
        Some('#') => 1,
        Some('b') => -1,
        _ => 0,
    };
    Some((base + shift).rem_euclid(12) as u8)
}

/// Reads ASCII tab (see the module documentation).
pub fn parse_ascii(text: &str, opts: &AsciiOptions) -> Result<AsciiParse, AsciiError> {
    if text.len() > MAX_ASCII_BYTES {
        return Err(AsciiError::TooLarge(text.len()));
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > MAX_ASCII_LINES {
        return Err(AsciiError::TooManyLines);
    }
    let parsed: Vec<Option<StringLine>> = lines.iter().map(|l| split_line(l)).collect();
    let mut warnings = Vec::new();
    let mut score = TabScore::new();
    score.tempo = vec![(0, opts.bpm)];
    score.meter = vec![(0, opts.numerator.max(1), opts.denominator.max(1))];
    // Title: the first line when a blank line follows it, or a `by artist` line and a blank.
    if let Some(first) = lines.iter().position(|l| !l.trim().is_empty())
        && parsed[first].is_none()
        && crate::sheet::section_name_of(lines[first]).is_none()
        && lines[first].chars().count() <= 80
    {
        let blank = |i: usize| lines.get(i).is_some_and(|l| l.trim().is_empty());
        if blank(first + 1) {
            score.title = lines[first].trim().to_owned();
        } else if let Some(artist) = lines
            .get(first + 1)
            .and_then(|l| l.trim().strip_prefix("by "))
            && blank(first + 2)
        {
            score.title = lines[first].trim().to_owned();
            score.artist = artist.trim().to_owned();
        }
    }
    let bar_len = bar_ticks(opts.numerator, opts.denominator);
    // One track per string count: (count, track, cursor tick).
    let mut tracks: Vec<(usize, TabTrack, i64)> = Vec::new();
    let mut i = 0usize;
    let mut systems = 0usize;
    let mut single = 0usize;
    'outer: while i < lines.len() {
        if parsed[i].is_none() {
            i += 1;
            continue;
        }
        // A run of string lines, split where the first label comes round again.
        let mut j = i;
        while j < lines.len() && parsed[j].is_some() {
            j += 1;
        }
        let labels_of_run: Vec<Option<String>> = parsed[i..j]
            .iter()
            .map(|l| l.as_ref().and_then(|l| l.label.clone()))
            .collect();
        let size = system_size(&labels_of_run);
        let mut k = i;
        while k < j {
            let end = (k + size).min(j);
            if end - k < 2 {
                single += 1;
                k = end;
                continue;
            }
            if systems >= MAX_SYSTEMS {
                warnings.push(format!("only the first {MAX_SYSTEMS} systems were read"));
                break 'outer;
            }
            systems += 1;
            let sys: Vec<&StringLine> = parsed[k..end].iter().flatten().collect();
            let above: Option<&str> = (k > 0).then(|| lines[k - 1]);
            let marker_line: Option<&str> = (k > 1).then(|| lines[k - 2]);
            let labels: Vec<Option<String>> = sys.iter().map(|l| l.label.clone()).collect();
            let n = sys.len();
            let slot = match tracks.iter().position(|t| t.0 == n) {
                Some(s) => s,
                None => {
                    let (tuning, id) = tuning_for(&labels, n, opts);
                    let name = if id.is_empty() {
                        format!("Tab ({n} strings)")
                    } else {
                        track_name(&id)
                    };
                    tracks.push((n, TabTrack::new(&name, &id, tuning), 0));
                    tracks.len() - 1
                }
            };
            let start_tick = tracks[slot].2;
            // A section name directly above (a palm-mute row may sit between).
            for cand in [above, marker_line].into_iter().flatten() {
                if let Some(name) = crate::sheet::section_name_of(cand) {
                    score.markers.push((start_tick, name));
                    break;
                }
                if !cand.trim().is_empty() && !cand.contains("PM") && !cand.contains("P.M") {
                    break;
                }
            }
            let pm = above
                .filter(|a| a.contains("PM") || a.contains("P.M"))
                .map(|a| palm_mute_spans(a, sys[0].start))
                .unwrap_or_default();
            let (notes, ticks) = parse_system(&sys, &pm, start_tick, bar_len, opts, &mut warnings);
            let repeats = sys[0]
                .trailing
                .split_whitespace()
                .find_map(repeat_count)
                .or_else(|| repeat_count(&sys[0].trailing))
                .unwrap_or(1);
            let track = &mut tracks[slot];
            for r in 0..i64::from(repeats) {
                for note in &notes {
                    if track.1.notes.len() >= limits::MAX_NOTES_PER_TRACK {
                        break;
                    }
                    let mut nn = note.clone();
                    nn.tick += r * ticks;
                    track.1.notes.push(nn);
                }
            }
            track.2 = start_tick + ticks * i64::from(repeats);
            let limit = bar_len.saturating_mul(i64::try_from(limits::MAX_BARS).unwrap_or(1));
            if track.2 > limit {
                warnings.push("the tab is too long; the rest was dropped".to_owned());
                break 'outer;
            }
            k = end;
        }
        i = j;
    }
    if single > 0 {
        warnings.push(format!(
            "{single} lone tab lines (a system needs at least two strings) were ignored"
        ));
    }
    if tracks.is_empty() {
        return Err(AsciiError::NoTab);
    }
    for (_, t, _) in tracks {
        score.tracks.push(t);
    }
    warnings.extend(score.sanitize());
    warnings.dedup();
    warnings.truncate(50);
    Ok(AsciiParse { score, warnings })
}

/// How many lines make one system in a run of string lines: the whole run when it is at most 12
/// lines, else the smallest period of its labels (two systems one above the other with no blank
/// line between), else 6.
fn system_size(labels: &[Option<String>]) -> usize {
    let n = labels.len();
    if labels.iter().all(Option::is_none) {
        return if n <= 12 { n } else { 6 };
    }
    for m in 2..n {
        if n.is_multiple_of(m) && (0..n).all(|x| labels[x] == labels[x % m]) {
            return m.min(12);
        }
    }
    if n <= 12 { n } else { 6 }
}

fn track_name(id: &str) -> String {
    find_instrument(id).map_or_else(|| "Tab".to_owned(), |i| i.name.to_owned())
}

/// Reads one system into notes; returns them and the ticks the system lasts.
fn parse_system(
    sys: &[&StringLine],
    pm: &[(usize, usize)],
    start_tick: i64,
    bar_len: i64,
    opts: &AsciiOptions,
    warnings: &mut Vec<String>,
) -> (Vec<TabNote>, i64) {
    let width = sys.iter().map(|l| l.body.len()).max().unwrap_or(0);
    // Bar boundaries: columns where at least half of the lines have a bar line.
    let need = sys.len().div_ceil(2).max(1);
    let is_bar: Vec<bool> = (0..width)
        .map(|c| sys.iter().filter(|l| l.body.get(c) == Some(&'|')).count() >= need)
        .collect();
    // Runs of bar columns (`||`, `|*`) count as one boundary, at their last column.
    let bounds: Vec<usize> = (0..width)
        .filter(|c| is_bar[*c] && !is_bar.get(c + 1).copied().unwrap_or(false))
        .collect();
    let mut bars: Vec<(usize, usize)> = Vec::new();
    let mut from = 0usize;
    for b in &bounds {
        if *b > from {
            bars.push((from, *b));
        }
        from = b + 1;
    }
    if from < width {
        bars.push((from, width));
    }
    let mut notes_out: Vec<TabNote> = Vec::new();
    let mut states: Vec<LineState> = sys.iter().map(|_| LineState::default()).collect();
    for (bar_index, (a, b)) in bars.iter().enumerate() {
        let (a, b) = (*a, *b);
        let mut raws: Vec<RawNote> = Vec::new();
        for (si, line) in sys.iter().enumerate() {
            scan_line(
                &line.body,
                a,
                b.min(line.body.len()),
                si,
                &mut states[si],
                &mut raws,
                warnings,
            );
        }
        let wb = i64::try_from(b - a).unwrap_or(1).max(1);
        let bar_start = start_tick + i64::try_from(bar_index).unwrap_or(0) * bar_len;
        let mut cols: Vec<usize> = raws.iter().map(|r| r.col).collect();
        cols.sort_unstable();
        cols.dedup();
        let tick_of = |col: usize| {
            bar_start + i64::try_from(col.saturating_sub(a)).unwrap_or(0) * bar_len / wb
        };
        let mut seen: Vec<(usize, usize)> = Vec::new();
        for r in raws {
            if seen.contains(&(r.string, r.col)) {
                continue;
            }
            seen.push((r.string, r.col));
            let tick = tick_of(r.col);
            let next_col = cols.iter().copied().find(|c| *c > r.col);
            let end = next_col.map_or(bar_start + bar_len, tick_of);
            let mut t = Techniques {
                flags: r.flags,
                slide: r.slide,
                vibrato: r.vibrato,
                bend: r.bend.clone(),
                ..Techniques::default()
            };
            if let Some(kind) = r.harmonic {
                t.harmonic = Some(Harmonic { kind, fret: None });
            }
            if pm.iter().any(|(s, e)| r.col >= *s && r.col < *e) {
                t.flags.insert(TechFlags::PALM_MUTE);
            }
            let mut n = TabNote::new(
                tick,
                (end - tick).max(1),
                u8::try_from(r.string).unwrap_or(0),
                r.fret,
            );
            n.tech = t;
            n.velocity = opts.velocity.clamp(1, 127);
            notes_out.push(n);
        }
    }
    let count = i64::try_from(bars.len()).unwrap_or(1).max(1);
    (notes_out, count * bar_len)
}

// ---------------------------------------------------------------------------------------------
// Writing

/// How the writer lays the text out.
#[derive(Debug, Clone, PartialEq)]
pub struct AsciiWriteOptions {
    /// The line width at which systems wrap.
    pub width: usize,
    /// Add a legend of the marks used after each track.
    pub legend: bool,
    /// Write the title and artist first.
    pub title: bool,
}

impl Default for AsciiWriteOptions {
    fn default() -> Self {
        Self {
            width: 80,
            legend: true,
            title: true,
        }
    }
}

const MAX_SLOTS: i64 = 64;

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

struct BarText {
    lines: Vec<Vec<char>>,
    annotation: Vec<char>,
    has_pm: bool,
    width: usize,
}

fn note_text(n: &TabNote) -> String {
    let t = &n.tech;
    let mut s = String::new();
    if t.has(TechFlags::DEAD) {
        s.push('x');
        return s;
    }
    let fret = n.fret.to_string();
    if t.has(TechFlags::TAP) {
        s.push('t');
    }
    match (&t.harmonic, t.has(TechFlags::GHOST)) {
        (Some(h), _) if h.kind == HarmonicKind::Natural => {
            s.push('<');
            s.push_str(&fret);
            s.push('>');
        }
        (Some(_), _) => {
            s.push('[');
            s.push_str(&fret);
            s.push(']');
        }
        (None, true) => {
            s.push('(');
            s.push_str(&fret);
            s.push(')');
        }
        (None, false) => {
            match t.slide {
                Some(SlideKind::InFromBelow) => s.push('/'),
                Some(SlideKind::InFromAbove) => s.push('\\'),
                _ => {}
            }
            s.push_str(&fret);
        }
    }
    if let Some(b) = &t.bend {
        if matches!(b.kind(), BendKind::Bend | BendKind::BendRelease) {
            let semis = (f64::from(b.peak()) / 100.0).round() as i32;
            let target = i32::from(n.fret) + semis.max(1);
            s.push('b');
            s.push_str(&target.to_string());
            if b.kind() == BendKind::BendRelease {
                s.push('r');
                s.push_str(&n.fret.to_string());
            }
        }
    } else if let Some(v) = t.vibrato {
        s.push_str(if v == VibratoKind::Wide { "~~" } else { "~" });
    }
    s
}

struct Cell<'a> {
    slot: i64,
    string: usize,
    note: &'a TabNote,
    text: String,
}

fn render_bar(track: &TabTrack, bar_start: i64, bar_len: i64, notes: &[&TabNote]) -> BarText {
    let n = track.tuning.len();
    let offs: Vec<i64> = notes
        .iter()
        .map(|x| (x.tick - bar_start).clamp(0, bar_len - 1))
        .collect();
    let mut g = bar_len;
    for o in &offs {
        g = gcd(g, *o);
    }
    let g = g.max(1);
    let coarse = bar_len / g > MAX_SLOTS;
    let slots = if coarse { MAX_SLOTS } else { bar_len / g };
    let slot_of = |o: i64| {
        if coarse {
            ((o * MAX_SLOTS + bar_len / 2) / bar_len).min(MAX_SLOTS - 1)
        } else {
            o / g
        }
    };
    let mut cells: Vec<Cell<'_>> = Vec::new();
    for (x, o) in notes.iter().zip(&offs) {
        let string = usize::from(x.string);
        if string >= n {
            continue;
        }
        let slot = slot_of(*o);
        if cells.iter().any(|c| c.slot == slot && c.string == string) {
            continue;
        }
        cells.push(Cell {
            slot,
            string,
            note: x,
            text: note_text(x),
        });
    }
    let maxlen = cells
        .iter()
        .map(|c| c.text.chars().count())
        .max()
        .unwrap_or(1);
    let slots_n = usize::try_from(slots).unwrap_or(1);
    // A bar is at least eight characters wide, so the line still reads as a tab line.
    let cw = (maxlen + 2).max(2).max(8usize.div_ceil(slots_n));
    let total = slots_n * cw;
    let mut lines: Vec<Vec<char>> = vec![vec!['-'; total]; n];
    let mut annotation: Vec<char> = vec![' '; total];
    for c in &cells {
        let at = usize::try_from(c.slot).unwrap_or(0) * cw;
        let len = c.text.chars().count();
        for (k, ch) in c.text.chars().enumerate() {
            lines[c.string][at + k] = ch;
        }
        // Slides and out-slides draw their mark right after the text.
        let later_fret = cells
            .iter()
            .filter(|o| o.string == c.string && o.slot > c.slot)
            .min_by_key(|o| o.slot)
            .map(|o| o.note.fret);
        let mark = match c.note.tech.slide {
            Some(SlideKind::Legato) | Some(SlideKind::Shift) => {
                Some(if later_fret.is_some_and(|f| f < c.note.fret) {
                    '\\'
                } else {
                    '/'
                })
            }
            Some(SlideKind::OutUp) => Some('/'),
            Some(SlideKind::OutDown) => Some('\\'),
            _ => None,
        };
        if let Some(m) = mark {
            lines[c.string][at + len] = m;
        }
        // The marker of a hammer-on or pull-off sits right before the destination.
        let flag = if c.note.tech.has(TechFlags::HAMMER_ON) {
            Some('h')
        } else if c.note.tech.has(TechFlags::PULL_OFF) {
            Some('p')
        } else {
            None
        };
        if let Some(m) = flag
            && at > 0
            && lines[c.string][at - 1] == '-'
            && cells
                .iter()
                .any(|o| o.string == c.string && o.slot < c.slot)
        {
            lines[c.string][at - 1] = m;
        }
    }
    // Palm mute: from the first muted slot to the end of the last one of each run.
    let muted = |s: i64| {
        let here: Vec<&Cell<'_>> = cells.iter().filter(|c| c.slot == s).collect();
        !here.is_empty() && here.iter().all(|c| c.note.tech.has(TechFlags::PALM_MUTE))
    };
    let mut has_pm = false;
    let mut slot = 0i64;
    while slot < slots {
        if !muted(slot) {
            slot += 1;
            continue;
        }
        let mut end = slot;
        let mut s = slot + 1;
        while s < slots {
            let any = cells.iter().any(|c| c.slot == s);
            if any && muted(s) {
                end = s;
            } else if any {
                break;
            }
            s += 1;
        }
        let from = usize::try_from(slot).unwrap_or(0) * cw;
        let to = ((usize::try_from(end).unwrap_or(0) + 1) * cw).min(total);
        annotation[from] = 'P';
        annotation[from + 1] = 'M';
        for ch in annotation.iter_mut().take(to).skip(from + 2) {
            *ch = '-';
        }
        has_pm = true;
        slot = end + 1;
    }
    BarText {
        lines,
        annotation,
        has_pm,
        width: total,
    }
}

fn label_for(track: &TabTrack, i: usize) -> String {
    let name = crate::instrument::pitch_class_name(track.tuning.strings[i].open).to_owned();
    if i == 0
        && track
            .tuning
            .strings
            .iter()
            .skip(1)
            .any(|s| crate::instrument::pitch_class_name(s.open) == name)
    {
        return name.to_lowercase();
    }
    name
}

/// Draws every non-percussion track of the score as ASCII tab (see the module documentation).
pub fn write_ascii(score: &TabScore, opts: &AsciiWriteOptions) -> String {
    let mut out = String::new();
    if opts.title && !score.title.is_empty() {
        out.push_str(&score.title.replace('\n', " "));
        out.push('\n');
        if !score.artist.is_empty() {
            out.push_str("by ");
            out.push_str(&score.artist.replace('\n', " "));
            out.push('\n');
        }
        out.push('\n');
    }
    let bars = score.bar_grid();
    let tracks: Vec<&TabTrack> = score
        .tracks
        .iter()
        .filter(|t| !t.is_percussion && !t.tuning.is_empty())
        .collect();
    for track in &tracks {
        if tracks.len() > 1 || !track.name.is_empty() {
            out.push_str(&format!("{}\n", track.name.replace('\n', " ")));
        }
        let labels: Vec<String> = (0..track.tuning.len())
            .map(|i| label_for(track, i))
            .collect();
        out.push_str(&format!(
            "Tuning: {}{}\n\n",
            labels.iter().rev().cloned().collect::<Vec<_>>().join(" "),
            if track.capo.fret > 0 {
                format!("  (capo {})", track.capo.fret)
            } else {
                String::new()
            }
        ));
        write_track(&mut out, track, &labels, &bars, &score.markers, opts);
    }
    out
}

fn write_track(
    out: &mut String,
    track: &TabTrack,
    labels: &[String],
    bars: &[(i64, i64)],
    markers: &[(i64, String)],
    opts: &AsciiWriteOptions,
) {
    let label_w = labels.iter().map(|l| l.chars().count()).max().unwrap_or(1);
    let mut rendered: Vec<(i64, BarText)> = Vec::new();
    for (start, len) in bars {
        let notes: Vec<&TabNote> = track
            .notes
            .iter()
            .filter(|n| !n.tie && n.tick >= *start && n.tick < *start + *len)
            .collect();
        let bt = if notes.is_empty() {
            BarText {
                lines: vec![vec!['-'; 16]; track.tuning.len()],
                annotation: vec![' '; 16],
                has_pm: false,
                width: 16,
            }
        } else {
            render_bar(track, *start, *len, &notes)
        };
        rendered.push((*start, bt));
    }
    let mut used: Vec<char> = Vec::new();
    let mut i = 0usize;
    let width = opts.width.max(label_w + 20);
    while i < rendered.len() {
        let mut j = i;
        let mut w = label_w + 1;
        while j < rendered.len() {
            let bw = rendered[j].1.width + 1;
            if j > i && w + bw > width {
                break;
            }
            w += bw;
            j += 1;
        }
        // Section markers that fall inside this system's bars.
        let first = rendered[i].0;
        let last = bars.get(j - 1).map_or(first, |b| b.0 + b.1);
        for (_, name) in markers.iter().filter(|(t, _)| *t >= first && *t < last) {
            out.push_str(&format!("[{}]\n", name.replace('\n', " ")));
        }
        let any_pm = rendered[i..j].iter().any(|(_, b)| b.has_pm);
        if any_pm {
            let mut row: String = " ".repeat(label_w + 1);
            for (_, b) in &rendered[i..j] {
                row.extend(b.annotation.iter());
                row.push(' ');
            }
            out.push_str(row.trim_end());
            out.push('\n');
            if !used.contains(&'M') {
                used.push('M');
            }
        }
        for (s, label) in labels.iter().enumerate() {
            let mut row = format!("{label:<label_w$}|");
            for (_, b) in &rendered[i..j] {
                row.extend(b.lines[s].iter());
                row.push('|');
            }
            for ch in row.chars().skip(label_w + 1) {
                if matches!(
                    ch,
                    'h' | 'p' | '/' | '\\' | 'b' | 'r' | '~' | 'x' | '(' | '<' | '[' | 't'
                ) && !used.contains(&ch)
                {
                    used.push(ch);
                }
            }
            out.push_str(&row);
            out.push('\n');
        }
        out.push('\n');
        i = j;
    }
    if opts.legend && !used.is_empty() {
        let mut parts: Vec<&str> = Vec::new();
        for c in &used {
            parts.push(match c {
                'h' => "h hammer-on",
                'p' => "p pull-off",
                '/' => "/ slide up",
                '\\' => "\\ slide down",
                'b' => "b bend",
                'r' => "r release",
                '~' => "~ vibrato",
                'x' => "x dead note",
                '(' => "(n) ghost note",
                '<' => "<n> harmonic",
                '[' => "[n] artificial harmonic",
                't' => "t tap",
                'M' => "PM palm mute",
                _ => continue,
            });
        }
        out.push_str(&format!("Legend: {}\n\n", parts.join(", ")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument::find_tuning;
    use crate::score::PPQ;

    const RIFF: &str = "Little Riff

[Intro]
e|-----------------|
B|-----------------|
G|--0-----0--------|
D|--------------2--|
A|--2-----2--------|
E|-----------------|
";

    fn parse(text: &str) -> AsciiParse {
        parse_ascii(text, &AsciiOptions::default()).unwrap()
    }

    #[test]
    fn reads_a_simple_system() {
        let p = parse(RIFF);
        let s = &p.score;
        assert_eq!(s.title, "Little Riff");
        assert_eq!(s.tracks.len(), 1);
        let t = &s.tracks[0];
        assert_eq!(t.instrument_id, "guitar6");
        assert_eq!(t.tuning.opens(), vec![64, 59, 55, 50, 45, 40]);
        assert_eq!(s.markers, vec![(0, "Intro".to_owned())]);
        let mut got: Vec<(u8, u8)> = t.notes.iter().map(|n| (n.string, n.fret)).collect();
        got.sort_unstable();
        assert_eq!(got, vec![(2, 0), (2, 0), (3, 2), (4, 2), (4, 2)]);
        // 17 columns in the bar; the first onset is at column 2.
        let first = t.notes.iter().map(|n| n.tick).min().unwrap();
        assert_eq!(first, 2 * 4 * PPQ / 17);
    }

    #[test]
    fn hammer_pull_slide_bend_vibrato_and_more() {
        let src = "e|-5h7p5---7b9---7b9r7--5/7--7\\5--12~--x--(5)--<12>--t9-|
B|--------------------------------------------------------|
";
        let p = parse(src);
        let notes = &p.score.tracks[0].notes;
        assert!(notes[1].tech.has(TechFlags::HAMMER_ON), "{notes:?}");
        assert!(notes[2].tech.has(TechFlags::PULL_OFF));
        let b = notes
            .iter()
            .find(|n| {
                n.tech
                    .bend
                    .as_ref()
                    .is_some_and(|b| b.kind() == BendKind::Bend)
            })
            .unwrap();
        assert_eq!(b.tech.bend.as_ref().map(Bend::peak), Some(200));
        assert!(notes.iter().any(|n| {
            n.tech
                .bend
                .as_ref()
                .is_some_and(|b| b.kind() == BendKind::BendRelease)
        }));
        assert!(
            notes
                .iter()
                .any(|n| n.tech.slide == Some(SlideKind::Legato))
        );
        assert!(
            notes
                .iter()
                .any(|n| n.tech.vibrato == Some(VibratoKind::Normal) && n.fret == 12)
        );
        assert!(notes.iter().any(|n| n.tech.has(TechFlags::DEAD)));
        assert!(
            notes
                .iter()
                .any(|n| n.tech.has(TechFlags::GHOST) && n.fret == 5)
        );
        assert!(notes.iter().any(|n| {
            n.tech
                .harmonic
                .is_some_and(|h| h.kind == HarmonicKind::Natural)
                && n.fret == 12
        }));
        assert!(
            notes
                .iter()
                .any(|n| n.tech.has(TechFlags::TAP) && n.fret == 9)
        );
    }

    #[test]
    fn slides_in_and_out() {
        let src = "e|-/5---7/---\\3---9\\-|
B|----------------------|
";
        let p = parse(src);
        let n = &p.score.tracks[0].notes;
        assert_eq!(n[0].tech.slide, Some(SlideKind::InFromBelow));
        assert_eq!(n[1].tech.slide, Some(SlideKind::OutUp));
        assert_eq!(n[2].tech.slide, Some(SlideKind::InFromAbove));
        assert_eq!(n[3].tech.slide, Some(SlideKind::OutDown));
    }

    #[test]
    fn palm_mute_annotation_and_repeats() {
        let src = "  PM----|
e|-0-0-0-0-|  x2
B|---------|
";
        let p = parse(src);
        let t = &p.score.tracks[0];
        assert_eq!(t.notes.len(), 8, "the system is repeated");
        let first_bar: Vec<_> = t.notes.iter().filter(|n| n.tick < 4 * PPQ).collect();
        assert_eq!(first_bar.len(), 4);
        assert!(first_bar.iter().any(|n| n.tech.has(TechFlags::PALM_MUTE)));
        assert!(t.notes.iter().any(|n| n.tick >= 4 * PPQ));
    }

    #[test]
    fn labels_pick_the_tuning() {
        let bass = "G|-0--0-|
D|-----2|
A|--2---|
E|------|
";
        let t = &parse(bass).score.tracks[0];
        assert_eq!(t.instrument_id, "bass4");
        let uke = "A|-3--0-|
E|-----1|
C|--0---|
G|------|
";
        let p = parse(uke);
        assert_eq!(p.score.tracks[0].tuning.opens(), vec![69, 64, 60, 67]);
        assert_eq!(p.score.tracks[0].instrument_id, "ukulele_soprano");
        let odd = "D|-0--0-|
A|-----2|
F|--2---|
";
        assert_eq!(parse(odd).score.tracks[0].tuning.len(), 3);
        let plain = "|-0--0-|
|-----2|
|--2---|
|------|
|------|
|------|
";
        assert_eq!(parse(plain).score.tracks[0].tuning.len(), 6);
    }

    #[test]
    fn two_string_counts_make_two_tracks() {
        let src = "e|-0---0-|
B|-----1-|
G|-------|
D|-------|
A|-------|
E|-------|

G|-0---0-|
D|-----2-|
A|-------|
E|-------|
";
        let p = parse(src);
        assert_eq!(p.score.tracks.len(), 2);
        assert_eq!(p.score.tracks[1].tuning.len(), 4);
    }

    #[test]
    fn multiple_systems_continue_in_time() {
        let src = "e|-0-0-0-0-|
B|---------|
G|---------|

e|-3-3-3-3-|
B|---------|
G|---------|
";
        let p = parse(src);
        let t = &p.score.tracks[0];
        assert_eq!(t.notes.len(), 8);
        assert!(t.notes[4].tick >= 4 * PPQ);
        assert_eq!(t.notes[4].fret, 3);
    }

    #[test]
    fn two_digit_frets_and_chords() {
        let src = "e|-12--|
B|-13--|
G|-12--|
";
        let p = parse(src);
        let n = &p.score.tracks[0].notes;
        assert_eq!(n.len(), 3);
        assert!(n.iter().all(|x| x.tick == n[0].tick));
        let mut f: Vec<u8> = n.iter().map(|x| x.fret).collect();
        f.sort_unstable();
        assert_eq!(f, vec![12, 12, 13]);
    }

    #[test]
    fn errors_and_garbage() {
        assert_eq!(
            parse_ascii("", &AsciiOptions::default()),
            Err(AsciiError::NoTab)
        );
        assert_eq!(
            parse_ascii("hello world\nthis is not a tab", &AsciiOptions::default()),
            Err(AsciiError::NoTab)
        );
        assert!(matches!(
            parse_ascii(&"x".repeat(MAX_ASCII_BYTES + 1), &AsciiOptions::default()),
            Err(AsciiError::TooLarge(_))
        ));
        for text in [
            "e|",
            "e|-|\nB|-|",
            "|||||\n|||||",
            "e|--999--|\nB|-------|",
            "e|-5b-|\nB|-5b-|",
            "e|--/--|\nB|--\\--|",
            "e|-(5-|\nB|-<5-|",
            "\u{0}\u{1}|----|\n|----|",
            "e|---h---|\nB|---p---|",
            "e|-0-|  x999999999999\nB|-0-|",
            "e|-0-|x2\nB|-0-|x2",
            "e|--0--|\nB|--0--",
            "e|\u{1f3b8}-0-|\nB|-0-|",
        ] {
            let _ = parse_ascii(text, &AsciiOptions::default());
        }
        let wide = format!("e|{}|\nB|{}|\n", "-1".repeat(100_000), "-".repeat(200_000));
        if let Ok(p) = parse_ascii(&wide, &AsciiOptions::default()) {
            assert!(p.score.note_count() <= limits::MAX_NOTES);
        }
    }

    #[test]
    fn many_repeats_stay_bounded() {
        let mut src = String::new();
        for _ in 0..200 {
            src.push_str("e|-0-0-0-0-0-0-0-0-0-0-0-0-0-0-0-0-|  x16\nB|----------------------------------|\n\n");
        }
        let p = parse_ascii(&src, &AsciiOptions::default()).unwrap();
        assert!(p.score.note_count() <= limits::MAX_NOTES);
    }

    fn guitar() -> Tuning {
        find_tuning("guitar6", "standard").unwrap().tuning.clone()
    }

    fn sample_score() -> TabScore {
        let mut s = TabScore::new();
        s.title = "Round Trip".into();
        s.artist = "Nobody".into();
        let mut t = TabTrack::new("Guitar", "guitar6", guitar());
        let q = PPQ;
        let e = PPQ / 2;
        // Bar 1: eighth notes with hammer, pull, slide, bend, vibrato, dead.
        t.notes.push(TabNote::new(0, e, 4, 2));
        let mut h = TabNote::new(e, e, 4, 4);
        h.tech.flags.insert(TechFlags::HAMMER_ON);
        t.notes.push(h);
        let mut p = TabNote::new(2 * e, e, 4, 2);
        p.tech.flags.insert(TechFlags::PULL_OFF);
        t.notes.push(p);
        let mut sl = TabNote::new(3 * e, e, 3, 5);
        sl.tech.slide = Some(SlideKind::Legato);
        t.notes.push(sl);
        t.notes.push(TabNote::new(4 * e, e, 3, 7));
        let mut b = TabNote::new(5 * e, e, 1, 12);
        b.tech.bend = Some(Bend::up(200));
        t.notes.push(b);
        let mut v = TabNote::new(6 * e, e, 0, 3);
        v.tech.vibrato = Some(VibratoKind::Normal);
        t.notes.push(v);
        let mut d = TabNote::new(7 * e, e, 5, 0);
        d.tech.flags.insert(TechFlags::DEAD);
        t.notes.push(d);
        // Bar 2: a chord, a ghost note, a harmonic, a palm-muted pair.
        for (s, f) in [(5u8, 3u8), (4, 2), (3, 0), (2, 0)] {
            t.notes.push(TabNote::new(4 * q, q, s, f));
        }
        let mut g = TabNote::new(5 * q, q, 1, 5);
        g.tech.flags.insert(TechFlags::GHOST);
        t.notes.push(g);
        let mut hm = TabNote::new(6 * q, q, 0, 12);
        hm.tech.harmonic = Some(Harmonic {
            kind: HarmonicKind::Natural,
            fret: None,
        });
        t.notes.push(hm);
        for k in 0..2 {
            let mut pm = TabNote::new(7 * q + k * e, e, 5, 0);
            pm.tech.flags.insert(TechFlags::PALM_MUTE);
            t.notes.push(pm);
        }
        s.tracks.push(t);
        s.markers.push((0, "Verse".to_owned()));
        s.sanitize();
        s
    }

    fn summary(s: &TabScore) -> Vec<(i64, u8, u8, u32)> {
        let mask = (TechFlags::HAMMER_ON
            | TechFlags::PULL_OFF
            | TechFlags::DEAD
            | TechFlags::GHOST
            | TechFlags::PALM_MUTE
            | TechFlags::TAP)
            .0;
        let mut v: Vec<_> = s.tracks[0]
            .notes
            .iter()
            .map(|n| (n.tick, n.string, n.fret, n.tech.flags.0 & mask))
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn write_then_read_gives_the_same_notes() {
        let score = sample_score();
        let text = write_ascii(&score, &AsciiWriteOptions::default());
        assert!(text.contains("Round Trip"), "{text}");
        assert!(text.contains("Legend:"), "{text}");
        let opts = AsciiOptions::default();
        let back = parse_ascii(&text, &opts).unwrap();
        assert_eq!(back.score.tracks.len(), 1, "{text}");
        assert_eq!(summary(&back.score), summary(&score), "{text}");
        let tr = &back.score.tracks[0];
        assert!(
            tr.notes
                .iter()
                .any(|n| n.tech.slide == Some(SlideKind::Legato)),
            "{text}"
        );
        assert!(
            tr.notes
                .iter()
                .any(|n| n.tech.bend.as_ref().is_some_and(|b| b.peak() == 200))
        );
        assert!(
            tr.notes
                .iter()
                .any(|n| n.tech.vibrato == Some(VibratoKind::Normal))
        );
        assert!(tr.notes.iter().any(|n| n.tech.harmonic.is_some()));
        assert_eq!(
            back.score.markers.first().map(|m| m.1.as_str()),
            Some("Verse")
        );
        assert_eq!(back.score.title, "Round Trip");
        // Writing the re-read score gives notes that read the same again.
        let again = write_ascii(&back.score, &AsciiWriteOptions::default());
        let third = parse_ascii(&again, &opts).unwrap();
        assert_eq!(summary(&third.score), summary(&score));
    }

    #[test]
    fn wrapping_respects_the_width() {
        let mut s = TabScore::new();
        let mut t = TabTrack::new("g", "guitar6", guitar());
        for bar in 0..12i64 {
            for k in 0..4i64 {
                t.notes.push(TabNote::new(
                    bar * 4 * PPQ + k * PPQ,
                    PPQ,
                    2,
                    (bar % 10) as u8,
                ));
            }
        }
        s.tracks.push(t);
        let text = write_ascii(
            &s,
            &AsciiWriteOptions {
                width: 60,
                legend: false,
                title: false,
            },
        );
        assert!(text.lines().all(|l| l.chars().count() <= 60), "{text}");
        let back = parse_ascii(&text, &AsciiOptions::default()).unwrap();
        assert_eq!(summary(&back.score), summary(&s));
    }

    #[test]
    fn writer_skips_percussion_and_handles_empty() {
        let mut s = TabScore::new();
        let mut d = TabTrack::new("drums", "", guitar());
        d.is_percussion = true;
        d.notes.push(TabNote::new(0, PPQ, 0, 0));
        s.tracks.push(d);
        assert!(!write_ascii(&s, &AsciiWriteOptions::default()).contains('|'));
        assert_eq!(
            write_ascii(&TabScore::new(), &AsciiWriteOptions::default()),
            ""
        );
        let mut s = TabScore::new();
        s.tracks.push(TabTrack::new("g", "guitar6", guitar()));
        assert!(write_ascii(&s, &AsciiWriteOptions::default()).contains("e|"));
    }

    #[test]
    fn dense_bars_are_rounded_not_dropped() {
        let mut s = TabScore::new();
        let mut t = TabTrack::new("g", "guitar6", guitar());
        for k in 0..100i64 {
            t.notes.push(TabNote::new(
                k * (4 * PPQ / 100),
                10,
                (k % 6) as u8,
                (k % 12) as u8,
            ));
        }
        s.tracks.push(t);
        let text = write_ascii(&s, &AsciiWriteOptions::default());
        let back = parse_ascii(&text, &AsciiOptions::default()).unwrap();
        assert!(back.score.note_count() >= 60);
    }

    #[test]
    fn tab_line_detector() {
        assert!(looks_like_tab_line("e|---3---|"));
        assert!(looks_like_tab_line("  B|--0--1--|  "));
        assert!(looks_like_tab_line("|-0-0-0-|"));
        assert!(!looks_like_tab_line("hello world"));
        assert!(!looks_like_tab_line("- a list item"));
        assert!(!looks_like_tab_line("----"));
        assert!(!looks_like_tab_line("Em  G  D"));
    }

    #[test]
    fn repeat_counts() {
        assert_eq!(repeat_count("x2"), Some(2));
        assert_eq!(repeat_count("(x3)"), Some(3));
        assert_eq!(repeat_count("4x"), Some(4));
        assert_eq!(repeat_count("x1"), None);
        assert_eq!(repeat_count("x99"), None);
        assert_eq!(repeat_count("hello"), None);
        assert_eq!(repeat_count("2"), None);
    }
}
