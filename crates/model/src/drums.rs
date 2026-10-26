//! Drum generator patterns in the document.
//!
//! A [`DrumDoc`] holds up to [`MAX_PATTERNS`] [`DrumPatternDoc`]s of a device that takes drum
//! data (`Registry::takes_drums`): a name, a length, a resolution, a swing, [`DRUM_LANES`] lanes
//! (the key each plays, its role and its mute) and [`MAX_STEPS`] steps per lane. A pattern always
//! holds all of them, so its length can grow again without losing data; steps at or beyond
//! `length` are kept (and saved) but never played. [`crate::player_data_for`] derives the engine's
//! hand-over value from the document.

use atomik_device_sdk::{
    DRUM_LANES, DrumData, DrumLane, DrumPattern, DrumRole, DrumStep, PlayerData,
};

use crate::steps::{MAX_NAME_CHARS, MAX_PATTERNS, MAX_STEPS, SeqResolution};

/// One step of a drum lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrumStepDoc {
    /// Velocity, 0 = no hit, up to 127.
    pub velocity: u8,
    /// Probability in percent, 0..=100.
    pub prob: u8,
}

impl Default for DrumStepDoc {
    /// No hit, probability 100 %.
    fn default() -> Self {
        Self {
            velocity: 0,
            prob: 100,
        }
    }
}

impl DrumStepDoc {
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.velocity > 127 {
            return Err(format!("velocity {} is outside 0..=127", self.velocity));
        }
        if self.prob > 100 {
            return Err(format!("prob {} is outside 0..=100", self.prob));
        }
        Ok(())
    }
}

/// One lane of a drum pattern: the key it plays, its role and whether it is muted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrumLaneDoc {
    /// Key, 0..=127.
    pub key: u8,
    /// What the lane is (the generator weighs bar positions by it).
    pub role: DrumRole,
    /// A muted lane plays nothing.
    pub mute: bool,
}

impl DrumLaneDoc {
    /// The default lane `index`: key 36 + index (the pads of the drum machine), the role of that
    /// key, not muted.
    pub const fn default_for(index: usize) -> Self {
        let d = DrumLane::default_for(index);
        Self {
            key: d.key,
            role: d.role,
            mute: false,
        }
    }
}

/// One drum pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrumPatternDoc {
    /// Display name (at most [`MAX_NAME_CHARS`] characters).
    pub name: String,
    /// Steps played, 1..=64.
    pub length: u8,
    /// Step length.
    pub resolution: SeqResolution,
    /// Swing in percent, 0..=75.
    pub swing: u8,
    /// Exactly [`DRUM_LANES`] lanes.
    pub lanes: Vec<DrumLaneDoc>,
    /// `steps[lane]`: exactly [`DRUM_LANES`] rows of exactly [`MAX_STEPS`] steps.
    pub steps: Vec<Vec<DrumStepDoc>>,
}

impl DrumPatternDoc {
    /// An empty 16-step 1/16 pattern without swing on the default lanes, called `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            length: 16,
            resolution: SeqResolution::Sixteenth,
            swing: 0,
            lanes: (0..DRUM_LANES).map(DrumLaneDoc::default_for).collect(),
            steps: vec![vec![DrumStepDoc::default(); MAX_STEPS]; DRUM_LANES],
        }
    }

    /// Validates the pattern. On failure the message names the offending field, e.g.
    /// `steps[3][5].velocity 130 is outside 0..=127`.
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
        if self.lanes.len() != DRUM_LANES {
            return Err(format!(
                "lanes holds {} entries, exactly {DRUM_LANES} are required",
                self.lanes.len()
            ));
        }
        for (i, l) in self.lanes.iter().enumerate() {
            if l.key > 127 {
                return Err(format!("lanes[{i}].key {} is outside 0..=127", l.key));
            }
        }
        if self.steps.len() != DRUM_LANES {
            return Err(format!(
                "steps holds {} lanes, exactly {DRUM_LANES} are required",
                self.steps.len()
            ));
        }
        for (l, row) in self.steps.iter().enumerate() {
            if row.len() != MAX_STEPS {
                return Err(format!(
                    "steps[{l}] holds {} entries, exactly {MAX_STEPS} are required",
                    row.len()
                ));
            }
            for (i, s) in row.iter().enumerate() {
                s.check().map_err(|m| format!("steps[{l}][{i}].{m}"))?;
            }
        }
        Ok(())
    }

    /// The engine's view of the pattern (a valid pattern is assumed).
    pub fn to_data(&self) -> DrumPattern {
        let mut data = DrumPattern {
            length: self.length,
            resolution: self.resolution.into(),
            swing: self.swing,
            ..DrumPattern::default()
        };
        for (slot, l) in data.lanes.iter_mut().zip(&self.lanes) {
            *slot = DrumLane {
                key: l.key,
                role: l.role,
                mute: l.mute,
            };
        }
        for (row, doc) in data.steps.iter_mut().zip(&self.steps) {
            for (slot, s) in row.iter_mut().zip(doc) {
                *slot = DrumStep {
                    velocity: s.velocity,
                    prob: s.prob,
                };
            }
        }
        data
    }

    /// A pattern document from the engine's value (what the generator wrote), called `name`.
    pub fn from_data(data: &DrumPattern, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            length: data.length,
            resolution: data.resolution.into(),
            swing: data.swing,
            lanes: data
                .lanes
                .iter()
                .map(|l| DrumLaneDoc {
                    key: l.key,
                    role: l.role,
                    mute: l.mute,
                })
                .collect(),
            steps: data
                .steps
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|s| DrumStepDoc {
                            velocity: s.velocity,
                            prob: s.prob,
                        })
                        .collect()
                })
                .collect(),
        }
    }

    /// The pattern with every step reset (name, length, resolution, swing and lanes stay).
    pub fn cleared(&self) -> Self {
        Self {
            steps: vec![vec![DrumStepDoc::default(); MAX_STEPS]; DRUM_LANES],
            ..self.clone()
        }
    }
}

impl Default for DrumPatternDoc {
    /// The pattern of a new generator: empty, 16 steps at 1/16, called `Pattern 1`.
    fn default() -> Self {
        Self::new("Pattern 1")
    }
}

/// The patterns of one drum generator device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrumDoc {
    /// 1..=[`MAX_PATTERNS`] patterns; the device's `pattern` param (1-based) selects one.
    pub patterns: Vec<DrumPatternDoc>,
}

impl Default for DrumDoc {
    /// One empty 16-step 1/16 pattern.
    fn default() -> Self {
        Self {
            patterns: vec![DrumPatternDoc::default()],
        }
    }
}

impl DrumDoc {
    /// Validates the pattern count and every pattern. On failure the message names the pattern.
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
        let mut d = DrumData::default();
        for (slot, p) in d.patterns.iter_mut().zip(&self.patterns) {
            *slot = p.to_data();
        }
        d.count = self.patterns.len().clamp(1, MAX_PATTERNS) as u8;
        PlayerData::Drums(d)
    }
}
