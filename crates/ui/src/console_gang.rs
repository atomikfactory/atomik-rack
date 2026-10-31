//! Ganged edits: a control of a selected strip drives the same control of every other selected
//! strip. Faders and send levels move by the same number of dB (the strips keep their offsets),
//! switches (mute, solo, send on and pre) take the state of the clicked one.
//!
//! The values are computed from where the drag started, not from the last step, so pulling the
//! control back restores the strips exactly. When a strip would pass the top of its range the
//! whole gang stops there (the end stop): the offsets between the strips never change. A strip
//! at the bottom (off) stays off.

use crate::console::fader_db;
use atomik_core::{ParamDescriptor, Unit};

/// A send level (`send3.level`).
fn is_send(param: &str, suffix: &str) -> bool {
    param.starts_with("send") && param.ends_with(suffix)
}

/// The params that move by dB steps with the gang: the fader and the send levels.
pub(crate) fn is_level(param: &str) -> bool {
    param == "fader" || is_send(param, ".level")
}

/// The params that take the clicked state: mute, solo and the send switches.
pub(crate) fn is_switch(param: &str) -> bool {
    matches!(param, "mute" | "solo") || is_send(param, ".on") || is_send(param, ".pre")
}

/// Whether editing `param` of a selected strip edits it on the other selected strips too.
pub(crate) fn is_ganged(param: &str) -> bool {
    is_level(param) || is_switch(param)
}

/// The plain value of `p` that shows `db`, clamped to the range.
fn plain_at(p: &ParamDescriptor, db: f32) -> f32 {
    match p.unit {
        Unit::Db => p.clamp(db),
        _ => p.clamp(atomik_dsp::fader_pos_for_db(db)),
    }
}

/// The dB a plain value shows, when it is a finite level (a strip that is off has none).
fn finite_db(p: &ParamDescriptor, plain: f32) -> Option<f32> {
    fader_db(p, plain).filter(|d| d.is_finite())
}

/// The dB a plain value shows, -inf included; `None` for a control that is no level.
pub(crate) fn level_db(p: &ParamDescriptor, plain: f32) -> Option<f32> {
    fader_db(p, plain)
}

/// The values of the gang after the driving strip moved by `delta_db`: one per member of
/// `starts` (its descriptor and the value it had when the drag began), in the same order.
pub(crate) fn level_targets(starts: &[(&ParamDescriptor, f32)], delta_db: f32) -> Vec<f32> {
    if delta_db.is_nan() {
        return starts.iter().map(|(p, v)| p.clamp(*v)).collect();
    }
    // The gang stops when the strip nearest the top reaches it.
    let room = starts
        .iter()
        .filter_map(|(p, v)| {
            let top = fader_db(p, p.max)?;
            finite_db(p, *v).map(|d| top - d)
        })
        .fold(f32::INFINITY, f32::min);
    let delta = delta_db.min(room.max(0.0));
    starts
        .iter()
        .map(|(p, v)| match finite_db(p, *v) {
            Some(d) => plain_at(p, d + delta),
            None => p.clamp(*v),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::{ParamId, Taper};

    const FADER: ParamDescriptor = ParamDescriptor {
        id: ParamId(0),
        key: "fader",
        name: "Fader",
        min: 0.0,
        max: 1.0,
        default: 0.75,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    };
    const SEND: ParamDescriptor = ParamDescriptor {
        id: ParamId(1),
        key: "send1.level",
        name: "Send 1",
        min: -60.0,
        max: 6.0,
        default: 0.0,
        taper: Taper::Decibel,
        unit: Unit::Db,
        discrete: false,
    };

    fn dbs(values: &[f32]) -> Vec<f32> {
        values
            .iter()
            .map(|v| fader_db(&FADER, *v).unwrap())
            .collect()
    }

    #[test]
    fn the_names_of_the_ganged_params() {
        for p in ["fader", "send1.level", "send8.level"] {
            assert!(is_level(p) && is_ganged(p), "{p}");
        }
        for p in ["mute", "solo", "send2.on", "send2.pre"] {
            assert!(is_switch(p) && is_ganged(p), "{p}");
        }
        for p in [
            "pan",
            "width",
            "input.gain",
            "comp.threshold",
            "send1.level_cv_trim",
        ] {
            assert!(!is_ganged(p), "{p}");
        }
    }

    #[test]
    fn faders_move_by_the_same_number_of_db() {
        let starts = [(&FADER, 0.5), (&FADER, 0.6), (&FADER, 0.75)];
        let before = dbs(&[0.5, 0.6, 0.75]);
        let after = dbs(&level_targets(&starts, -3.0));
        for (b, a) in before.iter().zip(&after) {
            assert!((a - b + 3.0).abs() < 1e-3, "{b} -> {a}");
        }
    }

    #[test]
    fn the_gang_stops_when_the_highest_strip_reaches_the_top() {
        let starts = [(&FADER, 0.75), (&FADER, 0.9)];
        let after = level_targets(&starts, 6.0);
        assert!(
            (after[1] - 1.0).abs() < 1e-6,
            "the nearest strip is at the top"
        );
        let (a, b) = (
            fader_db(&FADER, after[0]).unwrap(),
            fader_db(&FADER, after[1]).unwrap(),
        );
        let (a0, b0) = (0.0, fader_db(&FADER, 0.9).unwrap());
        assert!(((b - a) - (b0 - a0)).abs() < 1e-3, "the offset stays");
    }

    #[test]
    fn a_strip_that_is_off_stays_off_and_going_back_restores_the_others() {
        let starts = [(&FADER, 0.0), (&FADER, 0.6)];
        let down = level_targets(&starts, -240.0);
        assert_eq!(down[0], 0.0);
        assert!(down[1] < 0.01, "far down: {}", down[1]);
        let back = level_targets(&starts, 0.0);
        assert_eq!(back, [0.0, 0.6]);
    }

    #[test]
    fn send_levels_clamp_to_their_range() {
        let starts = [(&SEND, -10.0), (&SEND, 0.0)];
        let up = level_targets(&starts, 20.0);
        assert_eq!(up, [-4.0, 6.0], "the gang stops at +6 dB");
        let down = level_targets(&starts, -70.0);
        assert_eq!(down, [-60.0, -60.0], "off at the bottom");
    }
}
