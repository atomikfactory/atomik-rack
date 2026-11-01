//! CV sequencer curves in the document.
//!
//! A [`CurveDoc`] holds, for up to [`MAX_CV_PATTERNS`] patterns of a device that takes curves
//! (`Registry::takes_curves`), two rows A and B of [`MAX_STEPS`] [`CurveStepDoc`]s each. Pattern
//! `i` of the curves goes with pattern `i` of the device's [`crate::StepSeqDoc`]; a pattern the
//! document holds no curves for plays flat zeros. Like the notes, a row always holds all
//! [`MAX_STEPS`] steps so a pattern's length can grow again without losing data.
//! [`crate::player_data_for`] derives the engine's hand-over value from both documents.

use atomik_device_sdk::{CURVE_ROWS, CurveStep};

use crate::steps::{MAX_CV_PATTERNS, MAX_STEPS};
use atomik_device_sdk::MAX_PATTERNS;

/// Largest magnitude of a curve value, in percent.
pub const CURVE_VALUE_MAX: i8 = 100;

/// One step of a curve row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CurveStepDoc {
    /// Value in percent, -100..=100.
    pub value: i8,
    /// Ramps linearly to the next step's value over this step instead of holding.
    pub slide: bool,
}

impl CurveStepDoc {
    fn check(&self) -> Result<(), String> {
        if (-CURVE_VALUE_MAX..=CURVE_VALUE_MAX).contains(&self.value) {
            Ok(())
        } else {
            Err(format!(
                "value {} is outside -{CURVE_VALUE_MAX}..={CURVE_VALUE_MAX}",
                self.value
            ))
        }
    }
}

impl From<CurveStepDoc> for CurveStep {
    fn from(s: CurveStepDoc) -> Self {
        Self {
            value: s.value,
            slide: s.slide,
        }
    }
}

/// The two curve rows of one pattern; each holds exactly [`MAX_STEPS`] steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurvePatternDoc {
    /// Row A, then row B.
    pub rows: [Vec<CurveStepDoc>; CURVE_ROWS],
}

impl Default for CurvePatternDoc {
    /// Flat zero rows.
    fn default() -> Self {
        Self {
            rows: std::array::from_fn(|_| vec![CurveStepDoc::default(); MAX_STEPS]),
        }
    }
}

impl CurvePatternDoc {
    /// Validates the rows. On failure the message names the field, e.g.
    /// `rows[1][3].value 120 is outside -100..=100`.
    pub fn check(&self) -> Result<(), String> {
        for (r, row) in self.rows.iter().enumerate() {
            if row.len() != MAX_STEPS {
                return Err(format!(
                    "rows[{r}] holds {} steps, exactly {MAX_STEPS} are required",
                    row.len()
                ));
            }
            for (i, s) in row.iter().enumerate() {
                s.check().map_err(|m| format!("rows[{r}][{i}].{m}"))?;
            }
        }
        Ok(())
    }

    /// Whether every step is a flat zero.
    pub fn is_flat(&self) -> bool {
        self.rows
            .iter()
            .flatten()
            .all(|s| *s == CurveStepDoc::default())
    }
}

/// The curves of one CV sequencer device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurveDoc {
    /// 1..=[`MAX_CV_PATTERNS`] patterns, by pattern index.
    pub patterns: Vec<CurvePatternDoc>,
}

impl Default for CurveDoc {
    /// One flat pattern.
    fn default() -> Self {
        Self {
            patterns: vec![CurvePatternDoc::default()],
        }
    }
}

impl CurveDoc {
    /// Validates the pattern count and every pattern; the message names the pattern.
    pub fn check(&self) -> Result<(), String> {
        if !(1..=MAX_CV_PATTERNS).contains(&self.patterns.len()) {
            return Err(format!(
                "{} patterns is outside 1..={MAX_CV_PATTERNS}",
                self.patterns.len()
            ));
        }
        for (i, p) in self.patterns.iter().enumerate() {
            p.check().map_err(|m| format!("pattern {i}: {m}"))?;
        }
        Ok(())
    }

    /// The curves of pattern `index`, flat when the document holds none.
    pub fn pattern(&self, index: usize) -> CurvePatternDoc {
        self.patterns.get(index).cloned().unwrap_or_default()
    }

    /// The engine's view: all [`MAX_PATTERNS`] slots, flat where the document holds none (a
    /// valid document is assumed).
    pub fn to_data(&self) -> [[[CurveStep; MAX_STEPS]; CURVE_ROWS]; MAX_PATTERNS] {
        let mut out = flat_data();
        for (slot, pattern) in out.iter_mut().zip(&self.patterns) {
            for (row_slot, row) in slot.iter_mut().zip(&pattern.rows) {
                for (s, d) in row_slot.iter_mut().zip(row) {
                    *s = (*d).into();
                }
            }
        }
        out
    }
}

/// Flat engine curves for every pattern slot.
pub(crate) fn flat_data() -> [[[CurveStep; MAX_STEPS]; CURVE_ROWS]; MAX_PATTERNS] {
    [[[CurveStep::default(); MAX_STEPS]; CURVE_ROWS]; MAX_PATTERNS]
}
