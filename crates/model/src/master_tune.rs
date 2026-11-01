//! Master tune: the reference pitch of the whole project, as a shift in cents around A = 440 Hz.
//!
//! [`RackDoc::tune_cents`] is project data with a part in the sound: every pitched device (the
//! synths, the drum machine, the sampler, the loop slicer, the granular device and the tuner
//! reference) plays that many cents away from equal temperament at A = 440. It is 0 for a new
//! project, saved as the root field `"tune_cents"` only when it is not 0 (project format 39, so
//! a project at 0 differs from the same file without the field in nothing) and edited with
//! [`crate::Command::SetMasterTune`], one undo step that reports [`crate::SyncOp::TuneChanged`].
//! Audio tracks and plugins are not affected: a plugin keeps its own tuning.

use crate::history::Edit;
use crate::patch::{JsonPath, f32_text, number};
use crate::{ModelError, RackDoc};
use serde_json::Value;
use std::fmt::Write as _;

/// The largest shift either way, in cents.
pub const MASTER_TUNE_MAX_CENTS: f32 = 100.0;

/// The ratio a shift of `cents` scales every frequency by: `2^(cents / 1200)`.
pub fn tune_ratio(cents: f32) -> f64 {
    (f64::from(cents) / 1200.0).exp2()
}

/// Whether `cents` is an accepted master tune: finite and within [`MASTER_TUNE_MAX_CENTS`].
pub fn tune_ok(cents: f32) -> bool {
    cents.is_finite() && cents.abs() <= MASTER_TUNE_MAX_CENTS
}

/// Plans `Command::SetMasterTune`: the value must be accepted and differ from the current one.
pub(crate) fn plan_set(doc: &RackDoc, cents: f32) -> Result<Edit, ModelError> {
    if !tune_ok(cents) {
        return Err(ModelError::OutOfRange(format!(
            "a master tune of {cents} cents is outside -{MASTER_TUNE_MAX_CENTS}..={MASTER_TUNE_MAX_CENTS}"
        )));
    }
    // `-0.0` is the same tune as 0: one spelling, so a save reads back to the same bytes.
    let cents = if cents == 0.0 { 0.0 } else { cents };
    if doc.tune_cents.to_bits() == cents.to_bits() {
        return Err(ModelError::BadScope(
            "the master tune is already that".into(),
        ));
    }
    Ok(Edit::Tune {
        old: doc.tune_cents,
        new: cents,
    })
}

/// Writes `"tune_cents"` (with its trailing comma and newline) after `"macros"`; nothing for 0.
pub(crate) fn write_tune(w: &mut String, cents: f32) {
    if cents != 0.0 {
        let _ = writeln!(w, "  \"tune_cents\": {},", f32_text(cents));
    }
}

/// Reads the project's `"tune_cents"` number.
pub(crate) fn load_tune(v: &Value, path: &JsonPath) -> Result<f32, ModelError> {
    let cents = number(v, path)? as f32;
    if !tune_ok(cents) {
        return Err(path.error(format!(
            "{cents} is outside -{MASTER_TUNE_MAX_CENTS}..={MASTER_TUNE_MAX_CENTS}"
        )));
    }
    Ok(if cents == 0.0 { 0.0 } else { cents })
}
