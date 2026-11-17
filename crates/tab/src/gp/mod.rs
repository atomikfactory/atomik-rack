//! Guitar Pro readers: `.gp3`, `.gp4`, `.gp5` (the binary formats), `.gpx` (a compressed container
//! of an XML score) and `.gp` (a zip of the same XML), all read into a [`TabScore`].
//!
//! [`read_gp`] looks at the first bytes, not at a file name, to tell the formats apart. Every format
//! is first read into a small neutral document (`dom`: master bars, tracks, bars, voices, beats
//! and notes, with the note values already in ticks) and one converter (`convert`) turns that into
//! the [`TabScore`]: it expands repeats and alternative endings into a linear timeline, moves the
//! hammer-on and pull-off marks to the destination note, joins ties and attaches the tempo, meter,
//! key and marker maps. This keeps the quirks of each file format out of the conversion.
//!
//! The readers only import. They never trust a count in the file: every size is checked against the
//! bytes that are left, and the caps in [`limits`] bound the file, what a container decompresses to,
//! the XML tree and the notes, so hostile input fails with a [`GpError`] instead of exhausting
//! memory.

mod binary;
mod container;
pub(crate) mod convert;
pub(crate) mod dom;
mod gpif;
pub(crate) mod xml;
pub(crate) mod zip;

use std::collections::BTreeMap;

use crate::score::TabScore;

/// Caps on what a Guitar Pro file may ask for.
pub mod limits {
    /// The largest file that is read (a real score is a few hundred kilobytes).
    pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
    /// The most a container may decompress to (all entries together).
    pub const MAX_DECOMPRESSED: usize = 128 * 1024 * 1024;
    /// Elements in the XML tree of a `.gpif` score.
    pub const MAX_XML_NODES: usize = 3_000_000;
    /// Nesting depth of the XML tree.
    pub const MAX_XML_DEPTH: usize = 64;
    /// Tracks of a score (the same cap as [`crate::score::limits::MAX_TRACKS`]).
    pub const MAX_TRACKS: usize = crate::score::limits::MAX_TRACKS;
    /// Master bars of a score.
    pub const MAX_BARS: usize = crate::score::limits::MAX_BARS;
    /// Beats in a whole score.
    pub const MAX_BEATS: usize = 2_000_000;
    /// Beats in one voice of one bar.
    pub const MAX_BEATS_PER_BAR: usize = 4096;
    /// Notes in a whole score.
    pub const MAX_NOTES: usize = crate::score::limits::MAX_NOTES;
    /// Ticks of the expanded timeline (repeats unrolled): 72 000 quarter notes, 10 hours at 120 bpm.
    pub const MAX_TICKS: i64 = 72_000 * crate::score::PPQ;
    /// Entries in a zip or in a `.gpx` file system that are looked at.
    pub const MAX_ENTRIES: usize = 4096;
}

/// A reading problem, with a message that is fit to show to the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GpError {
    /// The bytes are not a Guitar Pro file this program reads.
    #[error("this is not a Guitar Pro file that Atomik Rack can read")]
    UnknownFormat,
    /// A Guitar Pro version older or newer than the readers cover.
    #[error(
        "Guitar Pro files of version {0} are not supported; save the song as .gp5 or .gp and try again"
    )]
    UnsupportedVersion(String),
    /// The file is larger than the limit.
    #[error("the file is too large ({0} bytes; the limit is {1})")]
    TooLarge(usize, usize),
    /// The file ends before the score does.
    #[error("the file ends too early; it is damaged or was not fully copied")]
    Truncated,
    /// The file is not valid inside.
    #[error("the file is damaged: {0}")]
    Corrupt(String),
    /// The container (zip or `.gpx` archive) is unreadable.
    #[error("the Guitar Pro archive is unreadable: {0}")]
    Container(String),
    /// What the container holds is bigger than the limit (a decompression bomb).
    #[error("the file expands to more than {0} bytes and was refused")]
    TooBig(usize),
    /// The score is bigger than the limits for tracks, bars, beats or notes.
    #[error("the score is too big to import: {0}")]
    TooMany(&'static str),
    /// The score XML is not well formed.
    #[error("the score is not valid XML: {0}")]
    Xml(String),
    /// The archive does not hold a score.
    #[error("the archive holds no score")]
    NoScore,
}

/// Something the import could not carry over exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWarning {
    /// What happened, in a sentence.
    pub message: String,
    /// How many times it happened (a warning that would repeat for every note is counted).
    pub count: usize,
}

impl std::fmt::Display for ImportWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.count > 1 {
            write!(f, "{} ({} times)", self.message, self.count)
        } else {
            f.write_str(&self.message)
        }
    }
}

/// Collects warnings, counting the ones that repeat.
#[derive(Debug, Default)]
pub(crate) struct Warnings {
    counts: BTreeMap<String, usize>,
}

impl Warnings {
    /// Notes one occurrence of `message`.
    pub(crate) fn warn(&mut self, message: &str) {
        if self.counts.len() < 200 || self.counts.contains_key(message) {
            *self.counts.entry(message.to_owned()).or_insert(0) += 1;
        }
    }

    /// The collected warnings, in message order.
    pub(crate) fn finish(self) -> Vec<ImportWarning> {
        self.counts
            .into_iter()
            .map(|(message, count)| ImportWarning { message, count })
            .collect()
    }
}

/// The file formats [`read_gp`] tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpFormat {
    /// `.gp3`, `.gp4`, `.gp5`: the binary formats.
    Binary,
    /// `.gpx`: a bit-compressed archive (or a raw one) holding the XML score.
    Gpx,
    /// `.gp`: a zip holding the XML score.
    Zip,
    /// A bare XML score (`score.gpif`).
    Gpif,
}

/// Tells the format from the first bytes, or `None` when it is none of them.
pub fn sniff(bytes: &[u8]) -> Option<GpFormat> {
    if bytes.starts_with(b"BCFZ") || bytes.starts_with(b"BCFS") {
        return Some(GpFormat::Gpx);
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Some(GpFormat::Zip);
    }
    if binary::has_header(bytes) {
        return Some(GpFormat::Binary);
    }
    let head = bytes.get(..bytes.len().min(512)).unwrap_or(bytes);
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with("<?xml") || t.starts_with("<GPIF") {
        return Some(GpFormat::Gpif);
    }
    None
}

/// Reads a Guitar Pro file of any supported format.
///
/// Returns the score and what could not be carried over. The score has been through
/// [`TabScore::sanitize`], whose own warnings are included.
pub fn read_gp(bytes: &[u8]) -> Result<(TabScore, Vec<ImportWarning>), GpError> {
    if bytes.len() > limits::MAX_FILE_BYTES {
        return Err(GpError::TooLarge(bytes.len(), limits::MAX_FILE_BYTES));
    }
    let mut warnings = Warnings::default();
    let doc = match sniff(bytes).ok_or(GpError::UnknownFormat)? {
        GpFormat::Binary => binary::read(bytes, &mut warnings)?,
        GpFormat::Gpx => {
            let score = container::extract_score(bytes)?;
            gpif::read(&score, &mut warnings)?
        }
        GpFormat::Zip => {
            let score = zip::extract_score(bytes)?;
            gpif::read(&score, &mut warnings)?
        }
        GpFormat::Gpif => gpif::read(bytes, &mut warnings)?,
    };
    let mut score = convert::convert(doc, &mut warnings)?;
    for w in score.sanitize() {
        warnings.warn(&w);
    }
    Ok((score, warnings.finish()))
}
