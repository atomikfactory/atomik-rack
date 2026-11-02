//! Moving an automation clip into a note clip as a device-parameter lane (see
//! [`crate::param_lane`]).
//!
//! The automation clip is drawn and edited like any other, on the lane of its parameter; this
//! turns what it plays under a note clip into a parameter lane of that clip, so the movement
//! travels with the notes from then on. The automation clip goes away (the track lane would
//! override the clip's lane otherwise).

use crate::history::Edit;
use crate::loop_convert::content_pieces;
use crate::param_lane::{ParamLane, canonical_params, check_param_shape, check_param_targets};
use crate::song::{AutomationPoint, ClipContent, ClipId, LaneKind, MAX_TICK, automation_piece};
use crate::song_cmd::{bad, find_clip, range};
use crate::{ModelError, RackDoc};
use atomik_device_sdk::Registry;

/// Plans `AutomationIntoClip`: see the command docs. The result is one compound edit (the note
/// clip with its new lane, the automation clip removed) and the note clip's id.
pub(crate) fn automation_into_clip(
    doc: &RackDoc,
    registry: &Registry,
    automation: ClipId,
    into: ClipId,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let (auto_device, auto_lane, a) = find_clip(doc, automation)?;
    let (note_device, note_lane, n) = find_clip(doc, into)?;
    let ClipContent::Automation(points) = &a.content else {
        return Err(bad(format!(
            "clip {} is not an automation clip",
            automation.0
        )));
    };
    let param = match doc
        .song
        .track(&auto_device)
        .and_then(|t| t.lanes.get(auto_lane))
        .map(|l| &l.kind)
    {
        Some(LaneKind::Automation(param)) => param.clone(),
        _ => {
            return Err(bad(format!(
                "clip {} is not on an automation lane",
                automation.0
            )));
        }
    };
    if !n.content.is_notes() {
        return Err(bad(format!("clip {} is not a note clip", into.0)));
    }
    if n.loop_length.is_some() {
        return Err(bad(format!(
            "clip {} loops: convert the loop to separate clips first",
            into.0
        )));
    }
    if n.param_lanes
        .iter()
        .any(|l| l.device == auto_device && l.param == param)
    {
        return Err(bad(format!(
            "clip {} already has a lane for {param}: clear the lanes first",
            into.0
        )));
    }
    // What the automation plays inside the note clip, in the note clip's content ticks.
    let mut lane_points: Vec<AutomationPoint> = Vec::new();
    for (from, to, cs) in content_pieces(a) {
        let (s, e) = (from.max(n.start), to.min(n.end()));
        if s >= e || points.is_empty() {
            continue;
        }
        let shift = n.offset - n.start;
        lane_points.extend(
            automation_piece(points, (s + shift, e + shift), cs + (s - from))
                .into_iter()
                .map(|(tick, value, curve)| AutomationPoint { tick, value, curve }),
        );
    }
    if lane_points.is_empty() {
        return Err(bad(format!(
            "clip {} is not under clip {}",
            automation.0, into.0
        )));
    }
    let mut lanes = n.param_lanes.clone();
    lanes.push(ParamLane::new(auto_device.clone(), param, lane_points));
    let lanes = check_param_targets(doc, registry, canonical_params(lanes))?;
    check_param_shape(&lanes, MAX_TICK).map_err(range)?;
    let mut new = n.clone();
    new.param_lanes = lanes;
    let edits = vec![
        Edit::Clip {
            device: note_device,
            lane: note_lane,
            old: Some(n.clone()),
            new: Some(new),
        },
        Edit::Clip {
            device: auto_device,
            lane: auto_lane,
            old: Some(a.clone()),
            new: None,
        },
    ];
    Ok((Edit::Compound(edits), Some(into)))
}
