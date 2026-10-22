//! Helpers shared by the devices that take a CV input with a `*_cv_trim` amount
//! (effects, console channel). See "CV conventions" in `docs/EXTENDING.md`.
//!
//! Contract: the CV is sanitised (non-finite -> 0) and clamped to -1..+1, scaled by the trim
//! (-1..+1), then added to the parameter in its normalised 0..1 space (or applied in dB / pan
//! units for level and pan targets) and clamped to the parameter range.

use atomik_core::{BATCH, db_to_gain};

/// CV sample sanitised and clamped to -1..+1.
#[inline]
pub fn clean(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Mean of a CV batch (each sample sanitised and clamped first), for batch-rate controls.
#[inline]
pub fn batch_mean(b: &[f32; BATCH]) -> f32 {
    let mut s = 0.0f32;
    for &x in b {
        s += clean(x);
    }
    s / BATCH as f32
}

/// Linear gain of a level CV: `10^(db * cv / 20)`, where `db` is the trim times the fixed range.
#[inline]
pub fn db_gain(db: f32, cv: f32) -> f32 {
    db_to_gain(db * clean(cv))
}

/// `base + trim * cv` in normalised space, clamped to 0..1.
#[inline]
pub fn add_norm(base: f32, trim: f32, cv: f32) -> f32 {
    (base + trim * clean(cv)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_clamps_and_sanitises() {
        assert_eq!(clean(5.0), 1.0);
        assert_eq!(clean(-5.0), -1.0);
        assert_eq!(clean(f32::NAN), 0.0);
        assert_eq!(clean(f32::INFINITY), 0.0);
        assert_eq!(clean(0.25), 0.25);
    }

    #[test]
    fn batch_mean_and_add_norm() {
        assert_eq!(batch_mean(&[0.5; BATCH]), 0.5);
        assert_eq!(batch_mean(&[9.0; BATCH]), 1.0);
        assert_eq!(add_norm(0.5, 1.0, 0.25), 0.75);
        assert_eq!(add_norm(0.5, 1.0, 3.0), 1.0);
        assert_eq!(add_norm(0.5, -1.0, 3.0), 0.0);
        assert_eq!(add_norm(0.5, 0.0, 1.0), 0.5);
        assert!((db_gain(24.0, 0.5) - 10f32.powf(0.6)).abs() < 1e-4);
        assert_eq!(db_gain(24.0, f32::NAN), 1.0);
    }
}
