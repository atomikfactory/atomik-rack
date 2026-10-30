//! Warp on import: whether a freshly imported audio file follows the song tempo, and at which
//! source tempo and clip length.
//!
//! The decision is a pure function ([`import_warp`]) of what the file says about itself
//! ([`FileMeta`], read by the import worker) and its length. With the preference
//! [`WarpImports::Loops`] (the default) a file is warped when it carries a tempo (`acid`
//! chunk) or when its length is a clean whole number of bars at a plausible tempo; a
//! one-shot or a long recording is left as it is. [`WarpImports::All`] warps every file; one
//! nothing is known about plays unstretched at the song tempo and follows later tempo changes.
//! [`WarpImports::Off`] imports as is, bit for bit.
//!
//! The choice is recorded in docs/DECISIONS.md (warp on import default).

use atomik_audio_file::FileMeta;
use atomik_core::PPQ;
use atomik_model::{WARP_BPM_MAX, WARP_BPM_MIN, bpm_for_beats};

/// The bar counts tried for a file without a tempo.
const BAR_COUNTS: [u32; 4] = [1, 2, 4, 8];
/// The tempo range a length-derived tempo must fall in.
const LENGTH_BPM_MIN: f64 = 70.0;
const LENGTH_BPM_MAX: f64 = 180.0;
/// How far, in BPM, a length-derived tempo may be from a whole BPM: a loop cut for a whole
/// number of bars at a round tempo, with a little slack for the cut. Tight on purpose: any
/// length fits some bar count, so a loose bound would warp recordings that are not loops.
const WHOLE_BPM_SLACK: f64 = 0.1;
/// How far, relative, the tempo that the stated beat count implies may differ from the stated
/// tempo for the beat count to be trusted (a file with a tail rings longer than its beats).
const BEATS_AGREE: f64 = 0.02;

/// Which files are warped when imported (Settings, Workflow, **Warp on import**).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WarpImports {
    /// Import as is.
    Off,
    /// Warp files that carry a tempo or are a clean number of bars long.
    #[default]
    Loops,
    /// Warp every file.
    All,
}

impl WarpImports {
    /// Every choice, in menu order.
    pub const ALL: [Self; 3] = [Self::Off, Self::Loops, Self::All];

    /// The name saved in `workflow.json`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Loops => "loops",
            Self::All => "all",
        }
    }

    /// The choice saved as `id`, if it is one.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Loops => "Loops only",
            Self::All => "All files",
        }
    }

    /// The next choice in [`Self::ALL`], wrapping.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// How an imported clip is warped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImportWarp {
    /// The tempo of the file, BPM.
    pub source_bpm: f64,
    /// The clip length in ticks: the file at its own tempo, in song time.
    pub length: i64,
}

/// What to do with a file of `frames` frames at `rate` that says `meta`, imported into a song at
/// `song_bpm` with bars of `bar_ticks` ticks: the warp to apply, or `None` to import it as is.
pub fn import_warp(
    meta: &FileMeta,
    frames: u64,
    rate: u32,
    song_bpm: f64,
    bar_ticks: i64,
    mode: WarpImports,
) -> Option<ImportWarp> {
    if mode == WarpImports::Off || frames == 0 || rate == 0 || bar_ticks <= 0 {
        return None;
    }
    let beats_per_bar = bar_ticks as f64 / PPQ as f64;
    let found = if meta.one_shot {
        None
    } else {
        from_metadata(meta, frames, rate)
            .or_else(|| from_length(frames, rate, song_bpm, beats_per_bar))
    };
    let (source_bpm, beats) = match (found, mode) {
        (Some(f), _) => f,
        (None, WarpImports::All) => {
            let bpm = song_bpm.clamp(WARP_BPM_MIN, WARP_BPM_MAX);
            (bpm, frames as f64 * bpm / (60.0 * f64::from(rate)))
        }
        _ => return None,
    };
    let length = ((beats * PPQ as f64).round() as i64).max(1);
    Some(ImportWarp { source_bpm, length })
}

/// The tempo and the beats of the file from what it states.
fn from_metadata(meta: &FileMeta, frames: u64, rate: u32) -> Option<(f64, f64)> {
    let exact = meta
        .beats
        .and_then(|b| bpm_for_beats(frames, f64::from(b), rate).map(|bpm| (bpm, f64::from(b))));
    match (meta.tempo_bpm, exact) {
        (Some(bpm), Some((e, beats))) if (e - bpm).abs() / bpm <= BEATS_AGREE => Some((e, beats)),
        (Some(bpm), _) => {
            let bpm = bpm.clamp(WARP_BPM_MIN, WARP_BPM_MAX);
            Some((bpm, frames as f64 * bpm / (60.0 * f64::from(rate))))
        }
        (None, exact) => exact,
    }
}

/// The tempo and the beats of a file whose length is a whole number of bars at a round tempo
/// between 70 and 180 BPM; of several bar counts that fit, the tempo nearest the song's.
fn from_length(frames: u64, rate: u32, song_bpm: f64, beats_per_bar: f64) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    for bars in BAR_COUNTS {
        let beats = f64::from(bars) * beats_per_bar;
        let Some(bpm) = bpm_for_beats(frames, beats, rate) else {
            continue;
        };
        let whole = bpm.round();
        if !(LENGTH_BPM_MIN..=LENGTH_BPM_MAX).contains(&bpm)
            || (bpm - whole).abs() > WHOLE_BPM_SLACK
        {
            continue;
        }
        let distance = |b: f64| (b / song_bpm.max(1.0)).ln().abs();
        if best.is_none_or(|(b, _)| distance(bpm) < distance(b)) {
            best = Some((bpm, beats));
        }
    }
    best
}

#[cfg(test)]
#[path = "import_warp_tests.rs"]
mod tests;
