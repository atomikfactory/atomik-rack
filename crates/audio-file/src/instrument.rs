//! Sampled instruments from other formats, as one neutral description.
//!
//! [`crate::sfz`] and [`crate::sf2`] read an instrument file into an [`Instrument`]: the
//! samples it plays and the regions that map them onto the keyboard. Nothing here knows about
//! Atomik's zones; the app turns an [`Instrument`] into zones once the samples are in the
//! project's pool (resampled to the session rate), so every frame position below is in frames
//! of the sample **as stored in its source**.
//!
//! Importing is lenient and bounded: what Atomik cannot play is skipped and counted in
//! [`Instrument::warnings`], never an error, and every size that comes from the file is capped
//! ([`InstrumentLimits`]).

use crate::data::AudioData;
use std::path::PathBuf;

/// Caps on what an instrument file may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstrumentLimits {
    /// Largest instrument file read, in bytes (a SoundFont holds its samples).
    pub max_file_bytes: u64,
    /// Most regions kept.
    pub max_regions: usize,
    /// Most distinct samples kept.
    pub max_samples: usize,
}

impl Default for InstrumentLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1 << 30,
            max_regions: 4096,
            max_samples: 512,
        }
    }
}

/// The largest SFZ text read: the file is only opcodes.
pub const MAX_SFZ_BYTES: u64 = 8 << 20;

/// Where a sample of an instrument comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum SampleSource {
    /// An audio file next to the instrument file (an SFZ names its samples by path).
    File(PathBuf),
    /// Audio held inside the instrument file (a SoundFont), at its own sample rate.
    Embedded(Box<AudioData>),
}

/// One sample of an instrument.
#[derive(Debug, Clone, PartialEq)]
pub struct InstrumentSample {
    /// A display name (the file name, or the SoundFont's sample name).
    pub name: String,
    /// The audio.
    pub source: SampleSource,
}

/// An amplitude envelope in seconds. `sustain` is a linear level, 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionEnv {
    /// Attack time in seconds.
    pub attack: f32,
    /// Decay time in seconds.
    pub decay: f32,
    /// Sustain level, 0..=1.
    pub sustain: f32,
    /// Release time in seconds.
    pub release: f32,
}

/// A filter of one region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionFilter {
    /// 0 low-pass, 1 band-pass, 2 high-pass.
    pub mode: u8,
    /// Cutoff in Hz.
    pub cutoff: f32,
    /// Resonance, 0..=1.
    pub resonance: f32,
    /// Key tracking, 0..=1 (1 follows the key one octave per octave).
    pub key_track: f32,
    /// Filter envelope amount, -1..=1.
    pub env_amount: f32,
}

/// One region: a sample mapped onto a key and velocity range.
#[derive(Debug, Clone, PartialEq)]
pub struct InstrumentRegion {
    /// Index into [`Instrument::samples`].
    pub sample: usize,
    /// Lowest key, 0..=127.
    pub key_lo: u8,
    /// Highest key.
    pub key_hi: u8,
    /// Lowest velocity, 1..=127.
    pub vel_lo: u8,
    /// Highest velocity.
    pub vel_hi: u8,
    /// The key at which the sample sounds at its recorded pitch.
    pub root_key: u8,
    /// Tuning in cents, whole semitones and fine tune together (not limited here).
    pub tune_cents: f64,
    /// Gain in dB (not limited here).
    pub gain_db: f32,
    /// Pan, -1..=1.
    pub pan: f32,
    /// First frame played.
    pub start: u64,
    /// End frame (exclusive); `None` plays to the end of the sample.
    pub end: Option<u64>,
    /// Loop `(start, end exclusive)`; `None` plays without a loop.
    pub loop_range: Option<(u64, u64)>,
    /// A note-off does not stop the sound.
    pub one_shot: bool,
    /// Plays backwards.
    pub reverse: bool,
    /// Group number of the region, 0 for none (1-based in the order the file lists groups).
    pub group: u32,
    /// Velocity crossfade width in velocity steps.
    pub xfade: u8,
    /// Round-robin `(cycle length, position 0-based)`.
    pub round_robin: Option<(u8, u8)>,
    /// The region's own amplitude envelope.
    pub env: Option<RegionEnv>,
    /// The region's own filter.
    pub filter: Option<RegionFilter>,
}

/// An instrument read from a file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Instrument {
    /// The instrument's name.
    pub name: String,
    /// The samples the regions play.
    pub samples: Vec<InstrumentSample>,
    /// The regions, in the file's order.
    pub regions: Vec<InstrumentRegion>,
    /// What was left out or changed, for the user (an unknown opcode is one line for the whole
    /// file).
    pub warnings: Vec<String>,
}

/// Whether `path` names an instrument file this crate reads (`.sfz`, `.sf2`; any case).
pub fn is_instrument_path(path: &std::path::Path) -> bool {
    instrument_kind(path).is_some()
}

/// The kind of instrument file `path` names, by extension.
pub fn instrument_kind(path: &std::path::Path) -> Option<InstrumentKind> {
    let ext = path.extension()?.to_str()?;
    if ext.eq_ignore_ascii_case("sfz") {
        Some(InstrumentKind::Sfz)
    } else if ext.eq_ignore_ascii_case("sf2") {
        Some(InstrumentKind::Sf2)
    } else {
        None
    }
}

/// A supported instrument format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentKind {
    /// Text mapping of sample files.
    Sfz,
    /// SoundFont 2: samples and mapping in one file.
    Sf2,
}

/// Reads an instrument file of either kind ([`instrument_kind`]); SFZ sample paths are
/// resolved against the file's folder.
///
/// # Errors
/// An unreadable or malformed file, or one over `limits.max_file_bytes`.
pub fn read_instrument(
    path: &std::path::Path,
    limits: InstrumentLimits,
) -> crate::Result<Instrument> {
    match instrument_kind(path) {
        Some(InstrumentKind::Sfz) => crate::sfz::read_sfz(path, limits),
        Some(InstrumentKind::Sf2) => crate::sf2::read_sf2(path, limits),
        None => Err(crate::AudioFileError::Unsupported(
            "not an instrument file (.sfz or .sf2)".into(),
        )),
    }
}

/// A warning line that lists at most a few names of what was left out.
pub(crate) fn listed(what: &str, names: &std::collections::BTreeSet<String>) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    const SHOWN: usize = 8;
    let shown: Vec<&str> = names.iter().take(SHOWN).map(String::as_str).collect();
    let more = names.len().saturating_sub(SHOWN);
    let tail = if more > 0 {
        format!(" (and {more} more)")
    } else {
        String::new()
    };
    Some(format!("{what}: {}{tail}", shown.join(", ")))
}
