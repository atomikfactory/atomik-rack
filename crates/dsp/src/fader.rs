//! Console fader taper (decision): a cubic amplitude taper in fader position.
//!
//! `gain(p) = (p / P_UNITY)³` for a position `p` in `0..=1`, with `P_UNITY =`
//! [`FADER_UNITY_POS`] `= 0.75`:
//! * `p = 0` → gain 0 (−∞ dB);
//! * `p = 0.75` → exactly unity (0 dB);
//! * `p = 1` → `(4/3)³ = 2.370` → **+7.50 dB** ("about +8 dB").
//!
//! Resolution concentrates near unity: 1 % of travel around ¾ is ≈ 0.35 dB. Typing a dB value
//! applies the inverse taper, [`fader_pos_for_db`]. The console stores the *position* as the
//! param value (0..1, linear taper), so the UI draws the cap at the param's normalised value
//! and labels it with [`fader_db`].

use atomik_core::{VCA_MAX_DB, VCA_MIN_DB, db_to_gain};

/// Fader position of unity gain.
pub const FADER_UNITY_POS: f32 = 0.75;

/// Linear gain of a fader position (clamped to `0..=1`; NaN → 0).
#[inline]
pub fn fader_gain(pos: f32) -> f32 {
    let p = if pos.is_nan() {
        0.0
    } else {
        pos.clamp(0.0, 1.0)
    } / FADER_UNITY_POS;
    p * p * p
}

/// Gain in dB of a fader position (`-inf` at 0).
#[inline]
pub fn fader_db(pos: f32) -> f32 {
    20.0 * fader_gain(pos).log10()
}

/// Inverse taper: the position giving `db` (clamped to `0..=1`; `-inf` → 0).
#[inline]
pub fn fader_pos_for_db(db: f32) -> f32 {
    if db.is_nan() {
        return 0.0;
    }
    let p = FADER_UNITY_POS * 10f32.powf(db / 60.0);
    p.clamp(0.0, 1.0)
}

/// Linear gain of a VCA offset in dB: 1 at 0 dB (exactly), 0 at or below [`VCA_MIN_DB`] (the
/// VCA is off), the plain dB gain between; clamped to [`VCA_MAX_DB`]; NaN reads as 0 dB.
#[inline]
pub fn vca_gain(db: f32) -> f32 {
    if db.is_nan() || db == 0.0 {
        1.0
    } else if db <= VCA_MIN_DB {
        0.0
    } else {
        db_to_gain(db.min(VCA_MAX_DB))
    }
}

/// Linear gain of a fader position with a VCA offset: the member's own [`fader_gain`] times
/// [`vca_gain`], multiplicative. With an offset of exactly 0 dB it is [`fader_gain`] itself, bit
/// for bit, so a channel outside any VCA group is unchanged.
#[inline]
pub fn fader_gain_vca(pos: f32, vca_db: f32) -> f32 {
    let g = fader_gain(pos);
    if vca_db == 0.0 || vca_db.is_nan() {
        g
    } else {
        g * vca_gain(vca_db)
    }
}

/// The effective level in dB of a member at fader position `pos` under a VCA at `vca_db`: the
/// two add (`-inf` when either is off).
#[inline]
pub fn fader_db_vca(pos: f32, vca_db: f32) -> f32 {
    20.0 * fader_gain_vca(pos, vca_db).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vca_offsets_add_in_db_and_zero_is_exact() {
        for pos in [0.1, 0.5, 0.75, 0.9, 1.0] {
            assert_eq!(
                fader_gain_vca(pos, 0.0).to_bits(),
                fader_gain(pos).to_bits()
            );
            let sum = fader_db(pos) - 6.0;
            assert!((fader_db_vca(pos, -6.0) - sum).abs() < 1e-3, "{pos}");
        }
        assert_eq!(vca_gain(VCA_MIN_DB), 0.0);
        assert_eq!(vca_gain(-80.0), 0.0);
        assert_eq!(fader_gain_vca(0.75, VCA_MIN_DB), 0.0);
        assert!(
            (vca_gain(100.0) - db_to_gain(VCA_MAX_DB)).abs() < 1e-6,
            "clamped"
        );
        assert_eq!(vca_gain(f32::NAN), 1.0);
        assert!(
            fader_db_vca(0.0, 6.0).is_infinite(),
            "an off fader stays off"
        );
    }

    #[test]
    fn fader_reference_points() {
        assert_eq!(fader_gain(0.0).to_bits(), 0.0f32.to_bits());
        assert!(fader_db(0.0).is_infinite() && fader_db(0.0) < 0.0);
        assert!(fader_db(0.75).abs() < 1e-5);
        assert!(
            (fader_db(1.0) - 60.0 * (4.0f32 / 3.0).log10()).abs() < 1e-4,
            "{}",
            fader_db(1.0)
        );
        assert!((fader_db(0.5) - 20.0 * (8.0f32 / 27.0).log10()).abs() < 1e-4);
        assert!((fader_gain(2.0) - fader_gain(1.0)).abs() < 1e-6, "clamped");
        assert_eq!(fader_gain(f32::NAN).to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn fader_inverse_round_trips() {
        for i in 1..=100 {
            let p = i as f32 / 100.0;
            let back = fader_pos_for_db(fader_db(p));
            assert!((back - p).abs() < 1e-5, "{p} -> {back}");
        }
        assert_eq!(
            fader_pos_for_db(f32::NEG_INFINITY).to_bits(),
            0.0f32.to_bits()
        );
        assert!(
            (fader_pos_for_db(40.0) - 1.0).abs() < 1e-6,
            "clamped at the top"
        );
    }

    #[test]
    fn fader_is_monotonic_and_fine_near_unity() {
        let mut prev = -1.0;
        for i in 0..=1000 {
            let g = fader_gain(i as f32 / 1000.0);
            assert!(g > prev || i == 0);
            prev = g;
        }
        let step = fader_db(0.76) - fader_db(0.75);
        assert!(step < 0.4, "1 % of travel near unity is {step} dB");
    }
}
