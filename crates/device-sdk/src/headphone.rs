//! Headphone data: the response (or correction) a user imported for the Headphone Correction
//! device, and the parser of the two text formats it reads.
//!
//! A [`HeadphoneData`] is built off the RT thread, shared by `Arc` as [`crate::PlayerData::Headphone`]
//! and handed to the device through [`crate::Device::swap_player_data`]; it is never mutated
//! afterwards. The project keeps the same value (see `RackDevice::headphone` in the model), so a
//! project does not depend on the file the data came from: [`HeadphoneData::name`] is a display
//! name only.
//!
//! # The two formats ([`parse`])
//!
//! * **Frequency response** (CSV or plain text): one `frequency, dB` pair per line, separated by a
//!   comma, a semicolon, a tab or spaces (with a semicolon, a comma is a decimal comma). A third
//!   column is the right channel (left in the second), unless the header names it `phase`. Lines
//!   that start with `#`, `*`, `//` or `%` are comments; text lines before the first number are
//!   headers; frequencies must rise. More than [`MAX_POINTS`] rows are averaged down onto a
//!   1/96-octave grid.
//! * **Parametric EQ text**: an optional `Preamp: -6.2 dB` line and one line per band, such as
//!   `Filter 1: ON PK Fc 105 Hz Gain -3.2 dB Q 0.70`. The types are `PK`/`PEQ` (peak),
//!   `LSC`/`LS`/`LSQ` (low shelf), `HSC`/`HS`/`HSQ` (high shelf), `LP`/`LPQ` and `HP`/`HPQ` (second
//!   order low and high pass). A missing Q is [`DEFAULT_Q`]; `OFF` bands are skipped.
//!
//! The parser is pure: every malformed input is an error naming the line, never a panic.

use std::fmt;

/// Most frequency points kept of a response.
pub const MAX_POINTS: usize = 4096;
/// Most bands of a parametric file.
pub const MAX_FILTERS: usize = 64;
/// Most bytes of text [`parse`] reads.
pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
/// Most characters of the display name.
pub const MAX_NAME_CHARS: usize = 128;
/// Q of a band that has none.
pub const DEFAULT_Q: f32 = 0.71;
/// Lowest frequency of a point or a band, Hz.
pub const MIN_FREQ: f32 = 1.0;
/// Highest frequency of a point or a band, Hz.
pub const MAX_FREQ: f32 = 200_000.0;
/// The level limit of a response value, a band gain and the preamp, dB.
pub const MAX_ABS_DB: f32 = 200.0;
/// Lowest Q of a band.
pub const MIN_Q: f32 = 0.05;
/// Highest Q of a band.
pub const MAX_Q: f32 = 100.0;
/// Points per octave of the grid a long response is averaged onto.
const THIN_PER_OCTAVE: f64 = 96.0;

/// What kind of band a parametric line describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    /// A bell: `gain` at `freq`.
    Peak,
    /// A low shelf: `gain` below `freq`.
    LowShelf,
    /// A high shelf: `gain` above `freq`.
    HighShelf,
    /// A second-order low pass (no gain).
    LowPass,
    /// A second-order high pass (no gain).
    HighPass,
}

impl FilterKind {
    /// The short name written to the project (`PK`, `LS`, `HS`, `LP`, `HP`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Peak => "PK",
            Self::LowShelf => "LS",
            Self::HighShelf => "HS",
            Self::LowPass => "LP",
            Self::HighPass => "HP",
        }
    }

    /// The kind of a short name, as [`FilterKind::name`] writes it.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "PK" => Self::Peak,
            "LS" => Self::LowShelf,
            "HS" => Self::HighShelf,
            "LP" => Self::LowPass,
            "HP" => Self::HighPass,
            _ => return None,
        })
    }

    /// Whether the band has a gain.
    pub const fn has_gain(self) -> bool {
        matches!(self, Self::Peak | Self::LowShelf | Self::HighShelf)
    }
}

/// One band of a parametric file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Filter {
    /// The kind of band.
    pub kind: FilterKind,
    /// Centre or corner frequency, Hz.
    pub freq: f32,
    /// Gain in dB (0 for a pass filter).
    pub gain_db: f32,
    /// Quality factor.
    pub q: f32,
}

/// A measured response (or a correction curve), as `frequency, dB` points.
#[derive(Debug, Clone, PartialEq)]
pub struct ResponseData {
    /// Frequencies in Hz, strictly rising.
    pub freq: Vec<f32>,
    /// The left channel in dB (the only one for a single curve).
    pub left: Vec<f32>,
    /// The right channel in dB when the file had one.
    pub right: Option<Vec<f32>>,
}

/// A parametric EQ: a correction as a preamp and bands.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterData {
    /// The file's preamp in dB (0 when it had none).
    pub preamp_db: f32,
    /// The bands, `OFF` ones left out.
    pub filters: Vec<Filter>,
}

/// What the file held.
#[derive(Debug, Clone, PartialEq)]
pub enum HeadphoneBody {
    /// A frequency response.
    Response(ResponseData),
    /// A parametric correction.
    Filters(FilterData),
}

/// Imported headphone data.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadphoneData {
    /// The display name (the file name it was imported from); never used to find a file.
    pub name: String,
    /// The data itself.
    pub body: HeadphoneBody,
}

impl HeadphoneData {
    /// Checks the value as the project loader and the commands do; the message says what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        let name_ok = !self.name.is_empty()
            && self.name.trim() == self.name
            && self.name.chars().count() <= MAX_NAME_CHARS
            && !self.name.chars().any(char::is_control);
        if !name_ok {
            return Err(format!(
                "the name must be trimmed and 1 to {MAX_NAME_CHARS} characters, without control characters"
            ));
        }
        match &self.body {
            HeadphoneBody::Response(r) => {
                let n = r.freq.len();
                if !(2..=MAX_POINTS).contains(&n) {
                    return Err(format!("{n} points is outside 2..={MAX_POINTS}"));
                }
                if r.left.len() != n || r.right.as_ref().is_some_and(|v| v.len() != n) {
                    return Err("every channel needs one level per frequency".to_owned());
                }
                let mut prev = 0.0_f32;
                for (i, f) in r.freq.iter().enumerate() {
                    if !(MIN_FREQ..=MAX_FREQ).contains(f) {
                        return Err(format!(
                            "frequency {f} (point {}) is outside {MIN_FREQ}..={MAX_FREQ} Hz",
                            i + 1
                        ));
                    }
                    if *f <= prev {
                        return Err(format!("frequency {f} (point {}) does not rise", i + 1));
                    }
                    prev = *f;
                }
                let mut levels = r.left.iter().chain(r.right.iter().flatten());
                if levels.any(|v| !v.is_finite() || v.abs() > MAX_ABS_DB) {
                    return Err(format!("a level is outside +-{MAX_ABS_DB} dB"));
                }
            }
            HeadphoneBody::Filters(f) => {
                if f.preamp_db.abs() > MAX_ABS_DB || !f.preamp_db.is_finite() {
                    return Err(format!("the preamp is outside +-{MAX_ABS_DB} dB"));
                }
                if f.filters.is_empty() && f.preamp_db.abs() < f32::MIN_POSITIVE {
                    return Err("a parametric file needs at least one band".to_owned());
                }
                if f.filters.len() > MAX_FILTERS {
                    return Err(format!(
                        "{} bands is more than the {MAX_FILTERS} allowed",
                        f.filters.len()
                    ));
                }
                for (i, b) in f.filters.iter().enumerate() {
                    let at = i + 1;
                    if !(MIN_FREQ..=MAX_FREQ).contains(&b.freq) {
                        return Err(format!("band {at}: the frequency is outside its range"));
                    }
                    if !(MIN_Q..=MAX_Q).contains(&b.q) {
                        return Err(format!("band {at}: Q is outside {MIN_Q}..={MAX_Q}"));
                    }
                    if !b.gain_db.is_finite() || b.gain_db.abs() > MAX_ABS_DB {
                        return Err(format!("band {at}: the gain is outside +-{MAX_ABS_DB} dB"));
                    }
                    if !b.kind.has_gain() && b.gain_db.abs() > 0.0 {
                        return Err(format!("band {at}: a pass filter has no gain"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether the data is a frequency response (so the device's `Data is` choice applies).
    pub fn is_response(&self) -> bool {
        matches!(self.body, HeadphoneBody::Response(_))
    }
}

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadphoneError {
    /// The 1-based line, 0 when the whole file is at fault.
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

impl HeadphoneError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for HeadphoneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            f.write_str(&self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for HeadphoneError {}

/// A display name for a path or a name: the file name without its folder, trimmed, control
/// characters removed and cut to [`MAX_NAME_CHARS`]; `"Headphones"` when nothing is left.
pub fn display_name(path_or_name: &str) -> String {
    let base = path_or_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path_or_name);
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let trimmed: String = cleaned.trim().chars().take(MAX_NAME_CHARS).collect();
    let trimmed = trimmed.trim().to_owned();
    if trimmed.is_empty() {
        "Headphones".to_owned()
    } else {
        trimmed
    }
}

/// Reads a headphone file's text (see the module docs for the two formats); `name` becomes the
/// display name (see [`display_name`]).
pub fn parse(text: &str, name: &str) -> Result<HeadphoneData, HeadphoneError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(HeadphoneError::at(
            0,
            format!("the file is larger than {} MiB", MAX_TEXT_BYTES >> 20),
        ));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let name = display_name(name);
    let body = if is_parametric(text) {
        HeadphoneBody::Filters(parse_filters(text)?)
    } else {
        HeadphoneBody::Response(parse_response(text)?)
    };
    let data = HeadphoneData { name, body };
    data.validate().map_err(|m| HeadphoneError::at(0, m))?;
    Ok(data)
}

fn is_comment(line: &str) -> bool {
    line.is_empty()
        || line.starts_with('#')
        || line.starts_with('*')
        || line.starts_with("//")
        || line.starts_with('%')
}

/// A file is parametric when a line starts with `Preamp` or `Filter` (any case).
fn is_parametric(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim().to_ascii_lowercase();
        l.starts_with("preamp") || l.starts_with("filter")
    })
}

/// The leading number of a token (`105Hz` is 105), `None` when it has none.
fn leading_number(token: &str) -> Option<f64> {
    let end = token
        .char_indices()
        .take_while(|(i, c)| {
            c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+'))
        })
        .last()
        .map(|(i, c)| i + c.len_utf8())?;
    let v: f64 = token[..end].parse().ok()?;
    // Anything after the number must be a unit (`Hz`, `dB`), not more digits or an exponent.
    let rest = &token[end..];
    (v.is_finite() && rest.chars().all(|c| c.is_ascii_alphabetic())).then_some(v)
}

fn parse_filters(text: &str) -> Result<FilterData, HeadphoneError> {
    let mut preamp = 0.0_f32;
    let mut filters = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if is_comment(line) {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("preamp") {
            let rest = &line["preamp".len()..];
            let value = rest
                .split(|c: char| c == ':' || c.is_whitespace() || c == '=')
                .find(|t| !t.is_empty())
                .and_then(leading_number)
                .ok_or_else(|| HeadphoneError::at(line_no, "the preamp has no number"))?;
            preamp = value as f32;
        } else if lower.starts_with("filter")
            && let Some(f) = parse_filter_line(line, line_no)?
        {
            if filters.len() == MAX_FILTERS {
                return Err(HeadphoneError::at(
                    line_no,
                    format!("more than {MAX_FILTERS} bands"),
                ));
            }
            filters.push(f);
        }
        // Any other line (a title, a remark) is ignored.
    }
    Ok(FilterData {
        preamp_db: preamp,
        filters,
    })
}

fn parse_filter_line(line: &str, line_no: usize) -> Result<Option<Filter>, HeadphoneError> {
    // `Filter 1: ON PK Fc 105 Hz Gain -3.2 dB Q 0.70`: what follows the colon.
    let body = line.split_once(':').map_or(line, |(_, rest)| rest);
    let tokens: Vec<&str> = body
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .collect();
    let mut i = 0;
    let mut on = true;
    if let Some(t) = tokens.first() {
        if t.eq_ignore_ascii_case("on") {
            i = 1;
        } else if t.eq_ignore_ascii_case("off") {
            on = false;
            i = 1;
        }
    }
    let kind_token = tokens
        .get(i)
        .ok_or_else(|| HeadphoneError::at(line_no, "the band has no type"))?;
    if !on {
        return Ok(None);
    }
    let kind = match kind_token.to_ascii_uppercase().as_str() {
        "PK" | "PEQ" => FilterKind::Peak,
        "LSC" | "LS" | "LSQ" => FilterKind::LowShelf,
        "HSC" | "HS" | "HSQ" => FilterKind::HighShelf,
        "LP" | "LPQ" => FilterKind::LowPass,
        "HP" | "HPQ" => FilterKind::HighPass,
        other => {
            return Err(HeadphoneError::at(
                line_no,
                format!("the band type {other} is not supported"),
            ));
        }
    };
    let (mut freq, mut gain, mut q) = (None, None, None);
    let mut j = i + 1;
    while j < tokens.len() {
        let key = tokens[j].to_ascii_lowercase();
        let slot = match key.as_str() {
            "fc" => Some(&mut freq),
            "gain" => Some(&mut gain),
            "q" => Some(&mut q),
            _ => None,
        };
        if let Some(slot) = slot {
            let v = tokens
                .get(j + 1)
                .and_then(|t| leading_number(t))
                .ok_or_else(|| {
                    HeadphoneError::at(line_no, format!("{} has no number", tokens[j]))
                })?;
            *slot = Some(v as f32);
            j += 2;
        } else {
            j += 1;
        }
    }
    let freq = freq.ok_or_else(|| HeadphoneError::at(line_no, "the band has no Fc"))?;
    let gain = if kind.has_gain() {
        gain.ok_or_else(|| HeadphoneError::at(line_no, "the band has no Gain"))?
    } else {
        0.0
    };
    let q = q.unwrap_or(DEFAULT_Q);
    if !(MIN_FREQ..=MAX_FREQ).contains(&freq) {
        return Err(HeadphoneError::at(
            line_no,
            format!("Fc {freq} is outside {MIN_FREQ}..={MAX_FREQ} Hz"),
        ));
    }
    if !(MIN_Q..=MAX_Q).contains(&q) {
        return Err(HeadphoneError::at(
            line_no,
            format!("Q {q} is outside {MIN_Q}..={MAX_Q}"),
        ));
    }
    if gain.abs() > MAX_ABS_DB {
        return Err(HeadphoneError::at(
            line_no,
            format!("Gain {gain} is outside +-{MAX_ABS_DB} dB"),
        ));
    }
    Ok(Some(Filter {
        kind,
        freq,
        gain_db: gain,
        q,
    }))
}

/// The numbers of one line of a response file, `None` when the line is not numeric.
fn numbers_of(line: &str) -> Option<Vec<f64>> {
    let semicolons = line.contains(';');
    let fixed;
    let line = if semicolons {
        // `;` separates the columns, so a comma is a decimal comma.
        fixed = line.replace(',', ".");
        fixed.as_str()
    } else {
        line
    };
    let mut out = Vec::new();
    for t in line.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        if t.is_empty() {
            continue;
        }
        let v: f64 = t.parse().ok()?;
        if !v.is_finite() {
            return None;
        }
        out.push(v);
    }
    (!out.is_empty()).then_some(out)
}

fn parse_response(text: &str) -> Result<ResponseData, HeadphoneError> {
    let mut freq: Vec<f32> = Vec::new();
    let mut left: Vec<f32> = Vec::new();
    let mut right: Vec<f32> = Vec::new();
    let mut columns = 0;
    let mut phase_header = false;
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if is_comment(line) {
            continue;
        }
        let nums = numbers_of(line);
        if columns == 0 {
            // Before the first data row a line that is not numbers is a header.
            match nums {
                Some(ref n) if n.len() >= 2 => {
                    columns = n.len().min(3);
                }
                Some(_) => {
                    return Err(HeadphoneError::at(
                        line_no,
                        "a line needs a frequency and a level",
                    ));
                }
                None => {
                    if line.to_ascii_lowercase().contains("phase") {
                        phase_header = true;
                    }
                    continue;
                }
            }
        }
        let n = nums.ok_or_else(|| HeadphoneError::at(line_no, "this line is not numbers"))?;
        if n.len() < columns {
            return Err(HeadphoneError::at(
                line_no,
                format!("{} numbers, the first row has {columns}", n.len()),
            ));
        }
        let f = n[0] as f32;
        if !(MIN_FREQ..=MAX_FREQ).contains(&f) {
            return Err(HeadphoneError::at(
                line_no,
                format!("frequency {f} is outside {MIN_FREQ}..={MAX_FREQ} Hz"),
            ));
        }
        if freq.last().is_some_and(|prev| f <= *prev) {
            return Err(HeadphoneError::at(
                line_no,
                format!("frequency {f} does not rise"),
            ));
        }
        for v in &n[1..columns] {
            if v.abs() > f64::from(MAX_ABS_DB) {
                return Err(HeadphoneError::at(
                    line_no,
                    format!("level {v} is outside +-{MAX_ABS_DB} dB"),
                ));
            }
        }
        freq.push(f);
        left.push(n[1] as f32);
        if columns == 3 {
            right.push(n[2] as f32);
        }
    }
    if freq.len() < 2 {
        return Err(HeadphoneError::at(
            0,
            "no frequency response found: expected lines of `frequency, dB`",
        ));
    }
    let right = (columns == 3 && !phase_header).then_some(right);
    let mut data = ResponseData { freq, left, right };
    if data.freq.len() > MAX_POINTS {
        data = thin(&data);
    }
    Ok(data)
}

/// Averages a long response down onto a 1/96-octave grid (the level of a cell is the mean of its
/// points in dB, its frequency their geometric mean), so a raw FFT export fits.
fn thin(r: &ResponseData) -> ResponseData {
    let cell = |f: f32| (f64::from(f).log2() * THIN_PER_OCTAVE).floor() as i64;
    let mut out = ResponseData {
        freq: Vec::new(),
        left: Vec::new(),
        right: r.right.as_ref().map(|_| Vec::new()),
    };
    let mut start = 0;
    while start < r.freq.len() {
        let c = cell(r.freq[start]);
        let mut end = start;
        while end < r.freq.len() && cell(r.freq[end]) == c {
            end += 1;
        }
        let n = (end - start) as f64;
        let mean =
            |v: &[f32]| (v[start..end].iter().map(|x| f64::from(*x)).sum::<f64>() / n) as f32;
        let log_mean = r.freq[start..end]
            .iter()
            .map(|x| f64::from(*x).ln())
            .sum::<f64>()
            / n;
        let f = log_mean.exp() as f32;
        // Rounding must not make two cells meet.
        if out.freq.last().is_none_or(|prev| f > *prev) {
            out.freq.push(f);
            out.left.push(mean(&r.left));
            if let (Some(o), Some(src)) = (out.right.as_mut(), r.right.as_ref()) {
                o.push(mean(src));
            }
        }
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(text: &str) -> ResponseData {
        match parse(text, "x.csv").unwrap().body {
            HeadphoneBody::Response(r) => r,
            HeadphoneBody::Filters(_) => panic!("parametric"),
        }
    }

    fn filters(text: &str) -> FilterData {
        match parse(text, "x.txt").unwrap().body {
            HeadphoneBody::Filters(f) => f,
            HeadphoneBody::Response(_) => panic!("response"),
        }
    }

    #[test]
    fn a_comma_separated_response_with_a_header_parses() {
        let r = response("Frequency, dB\n20, -1.5\n100, 0\n1000.5, 3.25\n");
        assert_eq!(r.freq, [20.0, 100.0, 1000.5]);
        assert_eq!(r.left, [-1.5, 0.0, 3.25]);
        assert!(r.right.is_none());
    }

    #[test]
    fn every_separator_and_comments_are_read() {
        let text =
            "# comment\n* REW style\n// more\n\nfreq\tspl\n20\t1\n  50   2  \n100;3\n200,4\n";
        // The semicolon line switches the decimal rule only for itself.
        let r = response(text);
        assert_eq!(r.freq, [20.0, 50.0, 100.0, 200.0]);
        assert_eq!(r.left, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn a_decimal_comma_works_with_semicolons() {
        let r = response("20,5;-3,25\n100;0,5\n");
        assert_eq!(r.freq, [20.5, 100.0]);
        assert_eq!(r.left, [-3.25, 0.5]);
    }

    #[test]
    fn three_columns_are_left_and_right_unless_the_header_says_phase() {
        let r = response("freq L R\n20 1 2\n100 3 4\n");
        assert_eq!(r.right.as_deref(), Some(&[2.0, 4.0][..]));
        let p = response("Freq(Hz) SPL(dB) Phase(degrees)\n20 1 90\n100 3 -45\n");
        assert!(p.right.is_none());
        assert_eq!(p.left, [1.0, 3.0]);
    }

    #[test]
    fn a_response_error_names_its_line() {
        let e = parse("20 1\n10 2\n", "x").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(e.message.contains("rise"), "{e}");
        let e = parse("20 1\n30 abc\n", "x").unwrap_err();
        assert_eq!(e.line, 2);
        let e = parse("20 1\n30\n", "x").unwrap_err();
        assert_eq!(e.line, 2);
        let e = parse("20 1\n0.2 3\n", "x").unwrap_err();
        assert!(e.to_string().starts_with("line 2"), "{e}");
        assert!(parse("", "x").is_err());
        assert!(parse("just words\nand more\n", "x").is_err());
        assert!(parse("20 1\n", "x").is_err());
        assert!(parse("20 1\n30 NaN\n", "x").is_err());
        assert!(parse("20 1\n30 inf\n", "x").is_err());
        assert!(parse("20 1\n30 9999\n", "x").is_err());
    }

    #[test]
    fn a_parametric_file_parses_with_preamp_shelves_and_pass_filters() {
        let f = filters(
            "Preamp: -6.2 dB\n\
             Filter 1: ON PK Fc 105 Hz Gain -3.2 dB Q 0.70\n\
             Filter 2: ON LSC Fc 60 Hz Gain 4.5 dB Q 0.9\n\
             Filter 3: ON HS Fc 8000 Hz Gain -2 dB\n\
             Filter 4: OFF PK Fc 1000 Hz Gain 9 dB Q 1\n\
             Filter 5: ON LP Fc 18000 Hz\n\
             Filter 6: ON HP Fc 25Hz Q 0.5\n\
             Filter 7: ON HSC Fc 10000 Hz Gain 1.5dB Q 0.71\n",
        );
        assert!((f.preamp_db + 6.2).abs() < 1e-6);
        assert_eq!(f.filters.len(), 6);
        assert_eq!(f.filters[0].kind, FilterKind::Peak);
        assert!((f.filters[0].q - 0.70).abs() < 1e-6);
        assert_eq!(f.filters[1].kind, FilterKind::LowShelf);
        assert_eq!(f.filters[2].kind, FilterKind::HighShelf);
        assert!((f.filters[2].q - DEFAULT_Q).abs() < 1e-6);
        assert_eq!(f.filters[3].kind, FilterKind::LowPass);
        assert!(f.filters[3].gain_db.abs() < 1e-9);
        assert_eq!(f.filters[4].kind, FilterKind::HighPass);
        assert!((f.filters[4].freq - 25.0).abs() < 1e-6);
        assert!((f.filters[5].gain_db - 1.5).abs() < 1e-6);
    }

    #[test]
    fn a_parametric_file_is_read_in_any_case_and_without_numbers_on_the_filter_word() {
        let f = filters("PREAMP -3dB\nfilter: on pk fc 1000 gain 2 q 1\n");
        assert!((f.preamp_db + 3.0).abs() < 1e-6);
        assert_eq!(f.filters.len(), 1);
    }

    #[test]
    fn a_parametric_error_names_its_line() {
        for (text, line) in [
            ("Filter 1: ON PK Fc 105 Hz Gain -3 dB Q 0\n", 1),
            ("Preamp: -2 dB\nFilter 1: ON NO Fc 1000 Q 1\n", 2),
            ("Filter 1: ON PK Gain 3 dB Q 1\n", 1),
            ("Filter 1: ON PK Fc 100 Hz Q 1\n", 1),
            ("Filter 1: ON PK Fc abc Gain 1 Q 1\n", 1),
            ("Preamp: loud\n", 1),
            ("Filter 1: ON\n", 1),
            ("Filter 1: ON PK Fc 100 Gain 900 Q 1\n", 1),
            ("Filter 1: ON PK Fc 0 Gain 1 Q 1\n", 1),
        ] {
            let e = parse(text, "x").unwrap_err();
            assert_eq!(e.line, line, "{text:?}: {e}");
        }
        // No bands and no preamp: nothing to correct with.
        assert!(parse("Filter 1: OFF PK Fc 100 Gain 1 Q 1\n", "x").is_err());
        let many: String = (0..=MAX_FILTERS)
            .map(|i| format!("Filter {i}: ON PK Fc 1000 Hz Gain 1 dB Q 1\n"))
            .collect();
        assert!(parse(&many, "x").is_err());
    }

    #[test]
    fn a_long_response_is_averaged_down_and_still_validates() {
        let mut text = String::new();
        for i in 0..30_000 {
            let f = 20.0 + f64::from(i) * 0.7;
            text.push_str(&format!("{f:.2} {:.2}\n", (f.ln() * 2.0).sin()));
        }
        let data = parse(&text, "raw.txt").unwrap();
        let HeadphoneBody::Response(r) = &data.body else {
            panic!()
        };
        assert!(r.freq.len() <= MAX_POINTS, "{}", r.freq.len());
        assert!(r.freq.len() > 500);
        assert!(r.freq.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(data.validate(), Ok(()));
    }

    #[test]
    fn display_name_drops_the_folder_and_bad_characters() {
        assert_eq!(display_name("C:\\Users\\me\\My Cans.csv"), "My Cans.csv");
        assert_eq!(display_name("/home/me/a\tb.txt"), "ab.txt");
        assert_eq!(display_name("   "), "Headphones");
        assert_eq!(
            display_name(&"x".repeat(500)).chars().count(),
            MAX_NAME_CHARS
        );
    }

    #[test]
    fn a_byte_order_mark_and_windows_line_ends_are_fine() {
        let r = response("\u{feff}20,1\r\n30,2\r\n");
        assert_eq!(r.freq, [20.0, 30.0]);
    }

    #[test]
    fn oversized_text_is_refused() {
        let big = "1 1\n".repeat(MAX_TEXT_BYTES / 4 + 8);
        assert!(parse(&big, "x").is_err());
    }
}
