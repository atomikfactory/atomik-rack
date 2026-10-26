//! Pattern data for note-player devices.
//!
//! A [`PlayerData`] is built off the RT thread, shared by `Arc` and handed to a device through
//! [`crate::Device::swap_player_data`], like a [`crate::SampleMap`]. It is never mutated
//! afterwards; an edit is a new value. All arrays are fixed-size so a device reads it without
//! allocating or copying.

use std::sync::Arc;

/// Patterns per step sequencer.
pub const MAX_PATTERNS: usize = 8;
/// Steps per pattern (the pattern's `length` selects how many play).
pub const MAX_STEPS: usize = 64;

/// One step of a pattern. A step beyond the pattern length is kept but never played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepData {
    /// Whether the step plays.
    pub on: bool,
    /// Key, 0..=127.
    pub key: u8,
    /// Velocity, 1..=127.
    pub velocity: u8,
    /// Gate length in percent of the step, 1..=100.
    pub gate: u8,
    /// Probability in percent, 0..=100.
    pub prob: u8,
    /// Tied to the next step (the note keeps sounding into it).
    pub tie: bool,
}

impl Default for StepData {
    fn default() -> Self {
        Self {
            on: false,
            key: 60,
            velocity: 100,
            gate: 80,
            prob: 100,
            tie: false,
        }
    }
}

/// Step length of a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SeqResolution {
    /// 1/4.
    Quarter,
    /// 1/8.
    Eighth,
    /// 1/8 triplet.
    EighthTriplet,
    /// 1/16.
    #[default]
    Sixteenth,
    /// 1/16 triplet.
    SixteenthTriplet,
    /// 1/32.
    ThirtySecond,
    /// 1/32 triplet.
    ThirtySecondTriplet,
}

impl SeqResolution {
    /// Every resolution, coarse to fine.
    pub const ALL: [Self; 7] = [
        Self::Quarter,
        Self::Eighth,
        Self::EighthTriplet,
        Self::Sixteenth,
        Self::SixteenthTriplet,
        Self::ThirtySecond,
        Self::ThirtySecondTriplet,
    ];

    /// Length of one step in ticks (15 360 per quarter, all exact integers).
    pub const fn step_ticks(self) -> u32 {
        match self {
            Self::Quarter => 15_360,
            Self::Eighth => 7_680,
            Self::EighthTriplet => 5_120,
            Self::Sixteenth => 3_840,
            Self::SixteenthTriplet => 2_560,
            Self::ThirtySecond => 1_920,
            Self::ThirtySecondTriplet => 1_280,
        }
    }
}

/// One pattern: a length, a resolution, a swing and 64 steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternData {
    /// Steps played, 1..=[`MAX_STEPS`].
    pub length: u8,
    /// Step length.
    pub resolution: SeqResolution,
    /// Swing in percent, 0..=75.
    pub swing: u8,
    /// The steps (all [`MAX_STEPS`] are present).
    pub steps: [StepData; MAX_STEPS],
}

impl Default for PatternData {
    fn default() -> Self {
        Self {
            length: 16,
            resolution: SeqResolution::Sixteenth,
            swing: 0,
            steps: [StepData::default(); MAX_STEPS],
        }
    }
}

/// The patterns of one step sequencer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepSeqData {
    /// Pattern slots; only the first `count` are valid.
    pub patterns: [PatternData; MAX_PATTERNS],
    /// Number of valid patterns, 1..=[`MAX_PATTERNS`].
    pub count: u8,
}

impl Default for StepSeqData {
    fn default() -> Self {
        Self {
            patterns: [PatternData::default(); MAX_PATTERNS],
            count: 1,
        }
    }
}

impl StepSeqData {
    /// Pattern `index` (0-based), or `None` beyond `count`.
    pub fn pattern(&self, index: usize) -> Option<&PatternData> {
        (index < usize::from(self.count).min(MAX_PATTERNS)).then(|| &self.patterns[index])
    }
}

/// One step of a curve row of the CV sequencer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CurveStep {
    /// Value in percent, -100..=100.
    pub value: i8,
    /// Ramps linearly to the next step's value over this step instead of holding.
    pub slide: bool,
}

/// Curve rows per pattern of the CV sequencer (A and B).
pub const CURVE_ROWS: usize = 2;

/// The data of a CV sequencer: the step sequencer's note patterns plus, per pattern, two curve
/// rows of [`MAX_STEPS`] steps (a step at or beyond the pattern's length is kept, never played).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CvSeqData {
    /// The note row, played like the step sequencer's.
    pub notes: StepSeqData,
    /// `curves[pattern][row][step]`; all [`MAX_PATTERNS`] slots are present.
    pub curves: [[[CurveStep; MAX_STEPS]; CURVE_ROWS]; MAX_PATTERNS],
}

impl Default for CvSeqData {
    fn default() -> Self {
        Self {
            notes: StepSeqData::default(),
            curves: [[[CurveStep::default(); MAX_STEPS]; CURVE_ROWS]; MAX_PATTERNS],
        }
    }
}

/// Immutable data for a note-player device.
// The value only ever lives in an `Arc` ([`PlayerDataRef`]), so the size of the smaller variants
// costs nothing worth a second allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerData {
    /// Step sequencer patterns.
    StepSeq(StepSeqData),
    /// CV sequencer: note patterns and curve rows.
    CvSeq(CvSeqData),
}

impl PlayerData {
    /// The note patterns every variant carries.
    pub fn notes(&self) -> &StepSeqData {
        match self {
            Self::StepSeq(d) => d,
            Self::CvSeq(d) => &d.notes,
        }
    }

    /// Curve row `row` of pattern `pattern`; `None` for data without curves (the device then
    /// plays flat zeros) or beyond the arrays.
    pub fn curve(&self, pattern: usize, row: usize) -> Option<&[CurveStep; MAX_STEPS]> {
        match self {
            Self::StepSeq(_) => None,
            Self::CvSeq(d) => d.curves.get(pattern)?.get(row),
        }
    }
}

/// Shared player data, as it travels to a device.
pub type PlayerDataRef = Arc<PlayerData>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolutions_have_the_exact_tick_lengths() {
        let ticks: Vec<u32> = SeqResolution::ALL.iter().map(|r| r.step_ticks()).collect();
        assert_eq!(ticks, [15_360, 7_680, 5_120, 3_840, 2_560, 1_920, 1_280]);
        assert_eq!(SeqResolution::default(), SeqResolution::Sixteenth);
    }

    #[test]
    fn defaults_are_an_empty_16_step_pattern() {
        let d = StepSeqData::default();
        assert_eq!(d.count, 1);
        let p = d.pattern(0).unwrap();
        assert_eq!((p.length, p.swing, p.steps.len()), (16, 0, 64));
        assert!(p.steps.iter().all(|s| !s.on && s.prob == 100));
        assert!(d.pattern(1).is_none());
        assert!(d.pattern(usize::MAX).is_none());
    }

    #[test]
    fn pattern_respects_count_and_never_indexes_past_the_array() {
        let d = StepSeqData {
            count: 200,
            ..StepSeqData::default()
        };
        assert!(d.pattern(7).is_some());
        assert!(d.pattern(8).is_none());
    }

    #[test]
    fn cv_seq_defaults_to_flat_curves_and_one_pattern() {
        let d = CvSeqData::default();
        assert_eq!(d.notes, StepSeqData::default());
        assert!(d.curves.iter().flatten().flatten().all(|s| *s
            == CurveStep {
                value: 0,
                slide: false
            }));
    }

    #[test]
    fn player_data_reads_notes_and_curves_of_either_variant() {
        let plain = PlayerData::StepSeq(StepSeqData::default());
        assert_eq!(plain.notes().count, 1);
        assert!(plain.curve(0, 0).is_none());
        let mut cv = CvSeqData::default();
        cv.curves[2][1][5] = CurveStep {
            value: -40,
            slide: true,
        };
        let d = PlayerData::CvSeq(cv);
        assert_eq!(d.notes().count, 1);
        assert_eq!(d.curve(2, 1).unwrap()[5].value, -40);
        assert!(d.curve(MAX_PATTERNS, 0).is_none());
        assert!(d.curve(0, CURVE_ROWS).is_none());
    }

    #[test]
    fn data_is_shared_by_arc() {
        let r: PlayerDataRef = Arc::new(PlayerData::StepSeq(StepSeqData::default()));
        let s = Arc::clone(&r);
        assert!(Arc::ptr_eq(&r, &s));
        assert_eq!(*r, *s);
    }
}
