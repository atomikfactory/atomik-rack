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

/// Lanes per drum pattern (the sixteen pads of the drum machine).
pub const DRUM_LANES: usize = 16;

/// What a drum lane stands for; the generator weighs each bar position by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrumRole {
    /// Bass drum.
    #[default]
    Kick,
    /// Snare.
    Snare,
    /// Hand clap.
    Clap,
    /// Closed hi-hat.
    ClosedHat,
    /// Open hi-hat.
    OpenHat,
    /// Low tom.
    TomLow,
    /// Mid tom.
    TomMid,
    /// High tom.
    TomHigh,
    /// Cymbal (crash or ride).
    Cymbal,
    /// Any other percussion.
    Percussion,
}

impl DrumRole {
    /// Every role, in file order.
    pub const ALL: [Self; 10] = [
        Self::Kick,
        Self::Snare,
        Self::Clap,
        Self::ClosedHat,
        Self::OpenHat,
        Self::TomLow,
        Self::TomMid,
        Self::TomHigh,
        Self::Cymbal,
        Self::Percussion,
    ];

    /// The index in [`DrumRole::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The role at `index` in [`DrumRole::ALL`].
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// The name used in project files.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Kick => "kick",
            Self::Snare => "snare",
            Self::Clap => "clap",
            Self::ClosedHat => "closed_hat",
            Self::OpenHat => "open_hat",
            Self::TomLow => "tom_low",
            Self::TomMid => "tom_mid",
            Self::TomHigh => "tom_high",
            Self::Cymbal => "cymbal",
            Self::Percussion => "percussion",
        }
    }

    /// The role called `name` in a project file.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.name() == name)
    }

    /// Short name for lane headers.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Kick => "kick",
            Self::Snare => "snare",
            Self::Clap => "clap",
            Self::ClosedHat => "closed hat",
            Self::OpenHat => "open hat",
            Self::TomLow => "tom low",
            Self::TomMid => "tom mid",
            Self::TomHigh => "tom high",
            Self::Cymbal => "cymbal",
            Self::Percussion => "perc",
        }
    }

    /// The role of a key in the usual drum-kit key layout (kick 36, snare 38, hats 42 and 46,
    /// toms 41 to 50, cymbals 49 and 51); anything else is percussion.
    pub const fn for_key(key: u8) -> Self {
        match key {
            35 | 36 => Self::Kick,
            38 | 40 => Self::Snare,
            39 => Self::Clap,
            42 | 44 => Self::ClosedHat,
            46 => Self::OpenHat,
            41 | 43 => Self::TomLow,
            45 | 47 => Self::TomMid,
            48 | 50 => Self::TomHigh,
            49 | 51 | 52 | 55 | 57 | 59 => Self::Cymbal,
            _ => Self::Percussion,
        }
    }
}

/// One lane of a drum pattern: the key it plays, what it is and whether it is muted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrumLane {
    /// Key, 0..=127.
    pub key: u8,
    /// The lane's role.
    pub role: DrumRole,
    /// A muted lane plays nothing.
    pub mute: bool,
}

impl DrumLane {
    /// The default lane `index` (0..16): key 36 + index, the role of that key.
    pub const fn default_for(index: usize) -> Self {
        let key = 36 + (index % DRUM_LANES) as u8;
        Self {
            key,
            role: DrumRole::for_key(key),
            mute: false,
        }
    }
}

/// One step of a drum lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrumStep {
    /// Velocity, 0 = no hit, up to 127.
    pub velocity: u8,
    /// Probability in percent, 0..=100.
    pub prob: u8,
}

impl Default for DrumStep {
    fn default() -> Self {
        Self {
            velocity: 0,
            prob: 100,
        }
    }
}

/// One drum pattern: a length, a resolution, a swing, sixteen lanes of [`MAX_STEPS`] steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrumPattern {
    /// Steps played, 1..=[`MAX_STEPS`].
    pub length: u8,
    /// Step length.
    pub resolution: SeqResolution,
    /// Swing in percent, 0..=75.
    pub swing: u8,
    /// The lanes.
    pub lanes: [DrumLane; DRUM_LANES],
    /// `steps[lane][step]` (all [`MAX_STEPS`] are present).
    pub steps: [[DrumStep; MAX_STEPS]; DRUM_LANES],
}

impl Default for DrumPattern {
    fn default() -> Self {
        let mut lanes = [DrumLane::default_for(0); DRUM_LANES];
        let mut i = 0;
        while i < DRUM_LANES {
            lanes[i] = DrumLane::default_for(i);
            i += 1;
        }
        Self {
            length: 16,
            resolution: SeqResolution::Sixteenth,
            swing: 0,
            lanes,
            steps: [[DrumStep::default(); MAX_STEPS]; DRUM_LANES],
        }
    }
}

/// The patterns of one drum generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrumData {
    /// Pattern slots; only the first `count` are valid.
    pub patterns: [DrumPattern; MAX_PATTERNS],
    /// Number of valid patterns, 1..=[`MAX_PATTERNS`].
    pub count: u8,
}

impl Default for DrumData {
    fn default() -> Self {
        Self {
            patterns: [DrumPattern::default(); MAX_PATTERNS],
            count: 1,
        }
    }
}

impl DrumData {
    /// Pattern `index` (0-based), or `None` beyond `count`.
    pub fn pattern(&self, index: usize) -> Option<&DrumPattern> {
        (index < usize::from(self.count).min(MAX_PATTERNS)).then(|| &self.patterns[index])
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
    /// Drum generator patterns.
    Drums(DrumData),
}

impl PlayerData {
    /// The note patterns of the step and CV sequencer data; `None` for drum data.
    pub fn notes(&self) -> Option<&StepSeqData> {
        match self {
            Self::StepSeq(d) => Some(d),
            Self::CvSeq(d) => Some(&d.notes),
            Self::Drums(_) => None,
        }
    }

    /// The drum patterns; `None` for any other data.
    pub fn drums(&self) -> Option<&DrumData> {
        match self {
            Self::Drums(d) => Some(d),
            _ => None,
        }
    }

    /// Curve row `row` of pattern `pattern`; `None` for data without curves (the device then
    /// plays flat zeros) or beyond the arrays.
    pub fn curve(&self, pattern: usize, row: usize) -> Option<&[CurveStep; MAX_STEPS]> {
        match self {
            Self::StepSeq(_) | Self::Drums(_) => None,
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
        assert_eq!(plain.notes().unwrap().count, 1);
        assert!(plain.curve(0, 0).is_none());
        let mut cv = CvSeqData::default();
        cv.curves[2][1][5] = CurveStep {
            value: -40,
            slide: true,
        };
        let d = PlayerData::CvSeq(cv);
        assert_eq!(d.notes().unwrap().count, 1);
        assert_eq!(d.curve(2, 1).unwrap()[5].value, -40);
        assert!(d.curve(MAX_PATTERNS, 0).is_none());
        assert!(d.curve(0, CURVE_ROWS).is_none());
    }

    #[test]
    fn drum_defaults_are_sixteen_empty_lanes_on_the_pad_keys() {
        let d = DrumData::default();
        assert_eq!(d.count, 1);
        let p = d.pattern(0).unwrap();
        assert_eq!((p.length, p.swing), (16, 0));
        for (i, lane) in p.lanes.iter().enumerate() {
            assert_eq!(lane.key, 36 + i as u8);
            assert_eq!(lane.role, DrumRole::for_key(36 + i as u8));
            assert!(!lane.mute);
        }
        assert_eq!(p.lanes[0].role, DrumRole::Kick);
        assert_eq!(p.lanes[2].role, DrumRole::Snare);
        assert_eq!(p.lanes[3].role, DrumRole::Clap);
        assert_eq!(p.lanes[6].role, DrumRole::ClosedHat);
        assert_eq!(p.lanes[10].role, DrumRole::OpenHat);
        assert!(
            p.steps
                .iter()
                .flatten()
                .all(|s| s.velocity == 0 && s.prob == 100)
        );
        assert!(d.pattern(1).is_none());
        assert!(d.pattern(usize::MAX).is_none());
    }

    #[test]
    fn drum_roles_round_trip_through_their_index() {
        for (i, r) in DrumRole::ALL.iter().enumerate() {
            assert_eq!(r.index(), i);
            assert_eq!(DrumRole::from_index(i), Some(*r));
            assert_eq!(DrumRole::from_name(r.name()), Some(*r));
        }
        assert_eq!(DrumRole::from_index(DrumRole::ALL.len()), None);
    }

    #[test]
    fn player_data_reads_drums_and_has_no_notes_or_curves_for_them() {
        let d = PlayerData::Drums(DrumData::default());
        assert!(d.notes().is_none());
        assert!(d.curve(0, 0).is_none());
        assert_eq!(d.drums().unwrap().count, 1);
        assert!(
            PlayerData::StepSeq(StepSeqData::default())
                .drums()
                .is_none()
        );
    }

    #[test]
    fn data_is_shared_by_arc() {
        let r: PlayerDataRef = Arc::new(PlayerData::StepSeq(StepSeqData::default()));
        let s = Arc::clone(&r);
        assert!(Arc::ptr_eq(&r, &s));
        assert_eq!(*r, *s);
    }
}
