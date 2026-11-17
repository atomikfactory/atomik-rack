//! MusicXML (`.musicxml`, `.xml`, compressed `.mxl`), read into and written from a [`TabScore`].
//!
//! [`read_musicxml`] reads a partwise score: parts become tracks, notes with `string` and `fret` keep
//! them (MusicXML string 1 is the highest string = tab line 0), pitch-only parts are fretted by the
//! crate's fretter for the part's tuning (`staff-tuning`, else a guitar), repeats and endings are
//! unrolled like the Guitar Pro readers do. [`write_musicxml`] and [`write_mxl`] write the same
//! facts back, including `staff-tuning` and `capo`, so a score written and read again keeps its
//! tunings, strings, frets and techniques.
//!
//! # What is mapped
//!
//! | Atomik tab | MusicXML |
//! | --- | --- |
//! | string, fret | `technical/string`, `technical/fret` (fret as shown in the tab, above the capo) |
//! | tuning, capo | `staff-details/staff-tuning`, `staff-details/capo` (+ a private `miscellaneous-field`) |
//! | hammer-on, pull-off | `hammer-on`/`pull-off` `start` on the origin, `stop` on the destination |
//! | legato slide, shift slide | `slide`, `glissando` (start on the origin) |
//! | slide in/out | `scoop`, `plop`, `doit`, `falloff` |
//! | bends, pre-bend, release | `bend` (`bend-alter` in semitones, `pre-bend`, `release`) |
//! | whammy dive | `bend` with `with-bar` |
//! | harmonics | `harmonic` (`natural`, `artificial`) and `other-technical` for the other kinds |
//! | tap, thumb | `tap`, `pluck` `p` |
//! | palm mute | `play/mute` = `palm` (also read from "P.M." directions) |
//! | let ring, slap, pop, fade-in, vibrato | `other-technical` (vibrato also from `wavy-line`) |
//! | dead note, ghost note | `notehead` `x`, `notehead` with `parentheses="yes"` |
//! | accent, marcato, staccato | `accent`, `strong-accent`, `staccato` |
//! | tremolo picking | `ornaments/tremolo` |
//! | grace note | `grace` note before the main note |
//! | strum direction | `arpeggiate` `direction` |
//! | chord diagram | `harmony/frame` |
//! | chord symbol | `harmony` (`root`, `kind`, `bass`) |
//! | lyrics | `lyric` (a hyphen at the end of a syllable = `begin`/`middle`) |
//! | tempo | `sound tempo` + `metronome` |
//! | meter, key, markers | `time`, `key`, `rehearsal` |
//! | percussion | `unpitched` notes with `midi-unpitched` instruments |
//!
//! Every limit of the Guitar Pro readers applies (file size, decompressed size, XML nodes and
//! depth, parts, bars, notes) and a hostile file fails with a [`MusicXmlError`] instead of panicking.

mod read;
mod write;
mod zipw;

use crate::gp::{ImportWarning, xml::Xml, zip};
use crate::score::TabScore;

/// Caps on what a MusicXML file may ask for (the Guitar Pro caps, plus the divisions).
pub mod limits {
    /// The largest file that is read.
    pub const MAX_FILE_BYTES: usize = crate::gp::limits::MAX_FILE_BYTES;
    /// The most a compressed container may decompress to.
    pub const MAX_DECOMPRESSED: usize = crate::gp::limits::MAX_DECOMPRESSED;
    /// The largest `divisions` value that is read (ticks per quarter note are 15 360).
    pub const MAX_DIVISIONS: i64 = 1_000_000;
    /// The largest `divisions` the writer uses (it quantises to this grid beyond it).
    pub const MAX_WRITE_DIVISIONS: i64 = 3840;
}

/// A reading or writing problem, with a message that is fit to show to the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MusicXmlError {
    /// The bytes are not MusicXML.
    #[error("this is not a MusicXML file that Atomik Rack can read")]
    UnknownFormat,
    /// A timewise score (rare); only partwise is read.
    #[error("this MusicXML file is timewise; save it as partwise MusicXML and try again")]
    Timewise,
    /// The file is larger than the limit.
    #[error("the file is too large ({0} bytes; the limit is {1})")]
    TooLarge(usize, usize),
    /// The `.mxl` container is unreadable.
    #[error("the compressed MusicXML archive is unreadable: {0}")]
    Container(String),
    /// What the container holds is bigger than the limit (a decompression bomb).
    #[error("the file expands to more than {0} bytes and was refused")]
    TooBig(usize),
    /// The document is not well formed XML.
    #[error("the file is not valid XML: {0}")]
    Xml(String),
    /// The score is bigger than the limits for parts, bars or notes.
    #[error("the score is too big to import: {0}")]
    TooMany(&'static str),
    /// The score has no part to read or write.
    #[error("the score has no music in it")]
    NoMusic,
    /// A score the writer cannot express.
    #[error("the score cannot be written as MusicXML: {0}")]
    Write(String),
}

/// The flavours of MusicXML file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicXmlFormat {
    /// An XML document (`.musicxml`, `.xml`).
    Xml,
    /// A compressed zip container (`.mxl`).
    Mxl,
}

/// Tells MusicXML from the first bytes (the extension is not trusted), or `None`.
///
/// An XML file counts when its first 4 KiB name a `score-partwise` or `score-timewise` root (so a
/// plain `.xml` of any other kind, or a Guitar Pro `score.gpif`, is not claimed). A zip counts when
/// it holds `META-INF/container.xml`.
pub fn sniff(bytes: &[u8]) -> Option<MusicXmlFormat> {
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        let names = zip::names(bytes).ok()?;
        let has_container = names
            .iter()
            .any(|n| n.eq_ignore_ascii_case("META-INF/container.xml"));
        return has_container.then_some(MusicXmlFormat::Mxl);
    }
    let head = bytes.get(..bytes.len().min(4096)).unwrap_or(bytes);
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if !(t.starts_with("<?xml") || t.starts_with("<score-") || t.starts_with("<!")) {
        return None;
    }
    (t.contains("<score-partwise") || t.contains("<score-timewise")).then_some(MusicXmlFormat::Xml)
}

fn map_gp(e: crate::gp::GpError) -> MusicXmlError {
    use crate::gp::GpError;
    match e {
        GpError::TooBig(n) => MusicXmlError::TooBig(n),
        GpError::TooMany(what) => MusicXmlError::TooMany(what),
        GpError::Xml(m) => MusicXmlError::Xml(m),
        GpError::Container(m) => MusicXmlError::Container(m),
        GpError::NoScore => MusicXmlError::Container("the archive holds no score".into()),
        other => MusicXmlError::Container(other.to_string()),
    }
}

/// The score document of an `.mxl` container: the rootfile `META-INF/container.xml` names, else the
/// first XML entry outside `META-INF`.
fn extract_mxl(bytes: &[u8]) -> Result<Vec<u8>, MusicXmlError> {
    let container = zip::read_named(bytes, "META-INF/container.xml").map_err(map_gp)?;
    let mut path = None;
    if let Ok(doc) = Xml::parse(&container) {
        let root = doc.root();
        if let Some(files) = root.child("rootfiles") {
            // Prefer a rootfile that is a score, not a PDF or a picture.
            let first = files
                .all("rootfile")
                .filter_map(|r| r.attr("full-path"))
                .find(|p| {
                    let l = p.to_ascii_lowercase();
                    l.ends_with(".xml") || l.ends_with(".musicxml")
                });
            path = first.map(str::to_owned);
        }
    }
    let path = match path {
        Some(p) => p,
        None => zip::names(bytes)
            .map_err(map_gp)?
            .into_iter()
            .find(|n| {
                let l = n.to_ascii_lowercase();
                !l.starts_with("meta-inf/") && (l.ends_with(".xml") || l.ends_with(".musicxml"))
            })
            .ok_or(MusicXmlError::NoMusic)?,
    };
    zip::read_named(bytes, &path).map_err(map_gp)
}

/// Reads a MusicXML file (`.musicxml`, `.xml` or `.mxl`).
///
/// Returns the score and what could not be carried over. The score has been through
/// [`TabScore::sanitize`], whose own warnings are included.
pub fn read_musicxml(bytes: &[u8]) -> Result<(TabScore, Vec<ImportWarning>), MusicXmlError> {
    if bytes.len() > limits::MAX_FILE_BYTES {
        return Err(MusicXmlError::TooLarge(bytes.len(), limits::MAX_FILE_BYTES));
    }
    let doc_bytes;
    let xml_bytes: &[u8] = match sniff(bytes).ok_or(MusicXmlError::UnknownFormat)? {
        MusicXmlFormat::Xml => bytes,
        MusicXmlFormat::Mxl => {
            doc_bytes = extract_mxl(bytes)?;
            &doc_bytes
        }
    };
    read::read(xml_bytes)
}

/// Writes a partwise `.musicxml` document.
///
/// The document has repeats unrolled (there is no repeat structure in a [`TabScore`] beyond the
/// informational [`TabScore::bars`]). Returns the text and what could not be expressed exactly.
pub fn write_musicxml(score: &TabScore) -> Result<(String, Vec<ImportWarning>), MusicXmlError> {
    write::write(score)
}

/// Writes a compressed `.mxl` container (a zip with `META-INF/container.xml` and the score).
pub fn write_mxl(score: &TabScore) -> Result<(Vec<u8>, Vec<ImportWarning>), MusicXmlError> {
    let (text, warnings) = write::write(score)?;
    Ok((zipw::mxl(text.as_bytes()), warnings))
}
