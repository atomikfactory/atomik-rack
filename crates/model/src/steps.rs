//! Step sequencer patterns in the document.
//!
//! A [`StepSeqDoc`] holds up to [`MAX_PATTERNS`] [`PatternDoc`]s of a device that takes pattern
//! data (`Registry::takes_player_data`). A pattern always holds [`MAX_STEPS`] steps so its
//! length can grow again without losing data; steps at or beyond `length` are kept (and
//! saved) but never played. [`player_data_for`] derives the immutable engine hand-over value
//! ([`PlayerData`]) from the document, so the app and the renderer build it the same way.

use atomik_device_sdk::{
    CvSeqData, PatternData, PlayerData, PlayerDataRef, Registry, SeqResolution as SdkResolution,
    StepData, StepSeqData,
};
use std::sync::Arc;

use crate::RackDevice;
use crate::curves::{CurveDoc, flat_data};

/// Patterns per sequencer.
pub const MAX_PATTERNS: usize = 8;
/// Steps stored per pattern.
pub const MAX_STEPS: usize = 64;
/// Longest pattern name, in characters.
pub const MAX_NAME_CHARS: usize = 64;

/// Step length of a pattern. Not the clip grid's `StepResolution` (1/8, 1/16, 1/32).
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

    /// The name used in project files.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Quarter => "quarter",
            Self::Eighth => "eighth",
            Self::EighthTriplet => "eighth_triplet",
            Self::Sixteenth => "sixteenth",
            Self::SixteenthTriplet => "sixteenth_triplet",
            Self::ThirtySecond => "thirty_second",
            Self::ThirtySecondTriplet => "thirty_second_triplet",
        }
    }

    /// The resolution called `name` in a project file.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.name() == name)
    }
}

impl From<SeqResolution> for SdkResolution {
    fn from(r: SeqResolution) -> Self {
        match r {
            SeqResolution::Quarter => Self::Quarter,
            SeqResolution::Eighth => Self::Eighth,
            SeqResolution::EighthTriplet => Self::EighthTriplet,
            SeqResolution::Sixteenth => Self::Sixteenth,
            SeqResolution::SixteenthTriplet => Self::SixteenthTriplet,
            SeqResolution::ThirtySecond => Self::ThirtySecond,
            SeqResolution::ThirtySecondTriplet => Self::ThirtySecondTriplet,
        }
    }
}

impl From<SdkResolution> for SeqResolution {
    fn from(r: SdkResolution) -> Self {
        match r {
            SdkResolution::Quarter => Self::Quarter,
            SdkResolution::Eighth => Self::Eighth,
            SdkResolution::EighthTriplet => Self::EighthTriplet,
            SdkResolution::Sixteenth => Self::Sixteenth,
            SdkResolution::SixteenthTriplet => Self::SixteenthTriplet,
            SdkResolution::ThirtySecond => Self::ThirtySecond,
            SdkResolution::ThirtySecondTriplet => Self::ThirtySecondTriplet,
        }
    }
}

/// One step of a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepDoc {
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
    /// Tied to the next step.
    pub tie: bool,
}

impl Default for StepDoc {
    /// An off step on C4 at velocity 100, gate 80 %, probability 100 %, no tie.
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

impl StepDoc {
    /// Checks the ranges; the message names the field.
    fn check(&self) -> Result<(), String> {
        let ranged = |field: &str, v: u8, lo: u8, hi: u8| {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(format!("{field} {v} is outside {lo}..={hi}"))
            }
        };
        ranged("key", self.key, 0, 127)?;
        ranged("velocity", self.velocity, 1, 127)?;
        ranged("gate", self.gate, 1, 100)?;
        ranged("prob", self.prob, 0, 100)
    }
}

impl From<StepDoc> for StepData {
    fn from(s: StepDoc) -> Self {
        Self {
            on: s.on,
            key: s.key,
            velocity: s.velocity,
            gate: s.gate,
            prob: s.prob,
            tie: s.tie,
        }
    }
}

/// One pattern: a name, a length, a resolution, a swing and always [`MAX_STEPS`] steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternDoc {
    /// Display name (at most [`MAX_NAME_CHARS`] characters).
    pub name: String,
    /// Steps played, 1..=64.
    pub length: u8,
    /// Step length.
    pub resolution: SeqResolution,
    /// Swing in percent, 0..=75.
    pub swing: u8,
    /// Exactly [`MAX_STEPS`] steps; those at or beyond `length` are kept but never played.
    pub steps: Vec<StepDoc>,
}

impl PatternDoc {
    /// An empty 16-step 1/16 pattern without swing, called `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            length: 16,
            resolution: SeqResolution::Sixteenth,
            swing: 0,
            steps: vec![StepDoc::default(); MAX_STEPS],
        }
    }

    /// Validates the pattern. On failure the message names the offending field, e.g.
    /// `steps[3].velocity 0 is outside 1..=127`.
    pub fn check(&self) -> Result<(), String> {
        if self.name.chars().count() > MAX_NAME_CHARS {
            return Err(format!("name is longer than {MAX_NAME_CHARS} characters"));
        }
        if !(1..=MAX_STEPS).contains(&usize::from(self.length)) {
            return Err(format!("length {} is outside 1..={MAX_STEPS}", self.length));
        }
        if self.swing > 75 {
            return Err(format!("swing {} is outside 0..=75", self.swing));
        }
        if self.steps.len() != MAX_STEPS {
            return Err(format!(
                "steps holds {} entries, exactly {MAX_STEPS} are required",
                self.steps.len()
            ));
        }
        for (i, s) in self.steps.iter().enumerate() {
            s.check().map_err(|m| format!("steps[{i}].{m}"))?;
        }
        Ok(())
    }

    /// The engine's view of the pattern (a valid pattern is assumed).
    fn to_data(&self) -> PatternData {
        let mut data = PatternData {
            length: self.length,
            resolution: self.resolution.into(),
            swing: self.swing,
            ..PatternData::default()
        };
        for (slot, step) in data.steps.iter_mut().zip(&self.steps) {
            *slot = (*step).into();
        }
        data
    }
}

impl Default for PatternDoc {
    /// The pattern of a new sequencer: empty, 16 steps at 1/16, called `Pattern 1`.
    fn default() -> Self {
        Self::new("Pattern 1")
    }
}

/// The patterns of one step sequencer device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSeqDoc {
    /// 1..=[`MAX_PATTERNS`] patterns; the device's `pattern` param (1-based) selects one.
    pub patterns: Vec<PatternDoc>,
}

impl Default for StepSeqDoc {
    /// One empty 16-step 1/16 pattern.
    fn default() -> Self {
        Self {
            patterns: vec![PatternDoc::default()],
        }
    }
}

impl StepSeqDoc {
    /// Validates the pattern count and every pattern. On failure the message names the
    /// pattern, e.g. `pattern 2: swing 80 is outside 0..=75`.
    pub fn check(&self) -> Result<(), String> {
        if !(1..=MAX_PATTERNS).contains(&self.patterns.len()) {
            return Err(format!(
                "{} patterns is outside 1..={MAX_PATTERNS}",
                self.patterns.len()
            ));
        }
        for (i, p) in self.patterns.iter().enumerate() {
            p.check().map_err(|m| format!("pattern {i}: {m}"))?;
        }
        Ok(())
    }

    /// The engine hand-over value for these patterns (a valid document is assumed); unused
    /// pattern slots hold the SDK default.
    pub fn to_player_data(&self) -> PlayerData {
        PlayerData::StepSeq(self.to_step_seq_data())
    }

    /// The note patterns as the engine's [`StepSeqData`].
    pub fn to_step_seq_data(&self) -> StepSeqData {
        let mut seq = StepSeqData::default();
        for (slot, pattern) in seq.patterns.iter_mut().zip(&self.patterns) {
            *slot = pattern.to_data();
        }
        seq.count = self.patterns.len().clamp(1, MAX_PATTERNS) as u8;
        seq
    }
}

/// The pattern data the engine gets for `device`, or `None` when its type takes none. A
/// sequencer without stored `steps` gets the default (one empty pattern); a type that takes
/// curves (`Registry::takes_curves`) gets [`PlayerData::CvSeq`] with its curves (flat when none
/// are stored), any other [`PlayerData::StepSeq`].
pub fn player_data_for(device: &RackDevice, registry: &Registry) -> Option<PlayerDataRef> {
    if !registry.takes_player_data(&device.type_id) {
        return None;
    }
    let notes = device
        .steps
        .as_ref()
        .map_or_else(StepSeqDoc::default, Clone::clone)
        .to_step_seq_data();
    Some(Arc::new(if registry.takes_curves(&device.type_id) {
        PlayerData::CvSeq(CvSeqData {
            notes,
            curves: device
                .curves
                .as_ref()
                .map_or_else(flat_data, CurveDoc::to_data),
        })
    } else {
        PlayerData::StepSeq(notes)
    }))
}
