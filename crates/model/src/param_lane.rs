//! Device-parameter lanes inside note clips.
//!
//! A note clip may carry [`ParamLane`]s next to its notes and controller lanes
//! (`Clip::param_lanes`): each holds the points of one parameter of one device, in the clip's
//! content ticks, with the [`AutomationPoint`] shape of an automation clip (`Linear`, `Step` or
//! `Bend` towards the next point). The lanes belong to the clip: they move, duplicate, split and
//! loop with it, and in a loop each repetition plays them again.
//!
//! The flattening ([`crate::song::flatten_param_lanes`]) turns them into ordinary automation
//! pieces. A track's own automation lane of the same parameter wins: where a track lane exists
//! the clip's lane is not played at all (the engine takes the first lane it is given for a
//! parameter, and the track lanes come first).
//!
//! The model keeps the target as written. A lane whose device or parameter has gone is kept
//! and plays nothing; the commands check the target when a lane is set.

use crate::song::{AutomationPoint, Curve};
use crate::{DeviceKey, ModelError, RackDoc};
use atomik_device_sdk::Registry;

/// The most parameter lanes one clip carries.
pub const MAX_PARAM_LANES: usize = 32;

/// One lane of a device parameter in a note clip (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct ParamLane {
    /// The device whose parameter the lane drives (a path key for a device in a container).
    pub device: DeviceKey,
    /// The parameter's key in the device's descriptor.
    pub param: String,
    /// Points in content ticks, strictly increasing, plain values. Never empty in a stored clip
    /// (an empty lane is dropped).
    pub points: Vec<AutomationPoint>,
}

impl ParamLane {
    /// A lane of `param` of `device` with `points`.
    pub fn new(device: DeviceKey, param: impl Into<String>, points: Vec<AutomationPoint>) -> Self {
        Self {
            device,
            param: param.into(),
            points,
        }
    }
}

/// The lanes in canonical form: sorted by `(device, param)`, lanes with no points dropped.
pub fn canonical_params(mut lanes: Vec<ParamLane>) -> Vec<ParamLane> {
    lanes.retain(|l| !l.points.is_empty());
    lanes.sort_by(|a, b| (&a.device, &a.param).cmp(&(&b.device, &b.param)));
    lanes
}

/// Checks the shape of a clip's parameter lanes: at most [`MAX_PARAM_LANES`], sorted by
/// `(device, param)` without duplicates, points sorted by tick without duplicates, ticks in
/// `0..=max_tick`, values finite, bends inside their range. `Err` names the first fault.
pub(crate) fn check_param_shape(lanes: &[ParamLane], max_tick: i64) -> Result<(), String> {
    if lanes.len() > MAX_PARAM_LANES {
        return Err(format!(
            "a clip carries at most {MAX_PARAM_LANES} parameter lanes, found {}",
            lanes.len()
        ));
    }
    for (i, lane) in lanes.iter().enumerate() {
        if i > 0 && (&lanes[i - 1].device, &lanes[i - 1].param) >= (&lane.device, &lane.param) {
            return Err(format!(
                "parameter lanes must be sorted by device and param without duplicates (lane {i})"
            ));
        }
        if lane.points.is_empty() {
            return Err(format!("parameter lane {i} has no points"));
        }
        for (j, p) in lane.points.iter().enumerate() {
            if !(0..=max_tick).contains(&p.tick) {
                return Err(format!(
                    "parameter lane {i} point {j}: tick {} is outside 0..={max_tick}",
                    p.tick
                ));
            }
            if !p.value.is_finite() {
                return Err(format!("parameter lane {i} point {j}: value is not finite"));
            }
            if let Curve::Bend(n) = p.curve
                && n.unsigned_abs() > atomik_core::bend::BEND_MAX.unsigned_abs()
            {
                return Err(format!(
                    "parameter lane {i} point {j}: bend {n} is out of range"
                ));
            }
            if j > 0 && lane.points[j - 1].tick >= p.tick {
                return Err(format!(
                    "parameter lane {i}: points must be sorted by tick without duplicates (point {j})"
                ));
            }
        }
    }
    Ok(())
}

/// Checks the lanes against the rack, which a command does when it sets them: every device is in
/// the rack and has the param, and the values are clamped into the param's range (lanes are
/// returned in canonical form).
pub(crate) fn check_param_targets(
    doc: &RackDoc,
    registry: &Registry,
    lanes: Vec<ParamLane>,
) -> Result<Vec<ParamLane>, ModelError> {
    let mut lanes = canonical_params(lanes);
    for lane in &mut lanes {
        let device = crate::rules::resolve(doc, &lane.device)?;
        let desc = crate::rules::descriptor(registry, &device.type_id)?;
        let Some(param) = desc.param_by_key(&lane.param) else {
            return Err(ModelError::UnknownKey(lane.param.clone()));
        };
        for p in &mut lane.points {
            if p.value.is_nan() {
                return Err(crate::song_cmd::range("a point value is NaN"));
            }
            p.value = param.clamp(p.value);
        }
    }
    Ok(lanes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
        AutomationPoint { tick, value, curve }
    }

    fn lane(device: &str, param: &str, points: Vec<AutomationPoint>) -> ParamLane {
        ParamLane::new(DeviceKey(device.into()), param, points)
    }

    #[test]
    fn canonical_sorts_and_drops_empty_lanes() {
        let lanes = canonical_params(vec![
            lane("synth", "cutoff", vec![pt(0, 1.0, Curve::Step)]),
            lane("fx", "mix", vec![]),
            lane("delay", "time", vec![pt(0, 0.5, Curve::Step)]),
        ]);
        let keys: Vec<_> = lanes.iter().map(|l| l.device.0.as_str()).collect();
        assert_eq!(keys, ["delay", "synth"]);
    }

    #[test]
    fn the_shape_check_names_the_first_fault() {
        let ok = vec![lane(
            "synth",
            "cutoff",
            vec![pt(0, 0.0, Curve::Bend(10)), pt(10, 1.0, Curve::Step)],
        )];
        assert!(check_param_shape(&ok, 100).is_ok());
        let twice = vec![
            lane("synth", "cutoff", vec![pt(0, 0.0, Curve::Step)]),
            lane("synth", "cutoff", vec![pt(0, 0.0, Curve::Step)]),
        ];
        assert!(check_param_shape(&twice, 100).is_err());
        let late = vec![lane("synth", "cutoff", vec![pt(101, 0.0, Curve::Step)])];
        assert!(check_param_shape(&late, 100).is_err());
        let nan = vec![lane("synth", "cutoff", vec![pt(0, f32::NAN, Curve::Step)])];
        assert!(check_param_shape(&nan, 100).is_err());
        let bent = vec![lane("synth", "cutoff", vec![pt(0, 0.0, Curve::Bend(-128))])];
        assert!(check_param_shape(&bent, 100).is_err());
        let unsorted = vec![lane(
            "synth",
            "cutoff",
            vec![pt(5, 0.0, Curve::Step), pt(5, 1.0, Curve::Step)],
        )];
        assert!(check_param_shape(&unsorted, 100).is_err());
    }
}
