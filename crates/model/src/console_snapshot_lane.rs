//! The snapshot lane: an automation lane whose points recall mixer snapshots.
//!
//! The lane lives on the master's automation track under the key [`SNAPSHOT_LANE_KEY`], like a
//! group's lanes ([`crate::console_vca`]), and is made with `AddAutomationLane`. It is an
//! ordinary lane of stepped points in clips, so it is drawn in the arrangement, moves with the
//! clips (drag, copy, split, insert and remove bars, scale tempo), muted with the lane and
//! edited with the lane gestures, each one undo step. The value of a point is the id of the
//! snapshot (1 to [`MAX_SNAPSHOTS`]); the app recalls that snapshot when the playhead crosses
//! the point while the song plays (see [`RackDoc::snapshot_lane_events`]). Unlike a param lane it
//! drives no device, so the engine never sees it, and a bounce does not recall.
//!
//! As in a pattern lane, the first point of a clip acts from the start of the clip. A point
//! recalls only when its snapshot differs from the one of the point before it, so a ramp's
//! closing point or a run of equal points recalls once. A lane has no static value.

use crate::console::MASTER_TYPE;
use crate::console_snapshot::MAX_SNAPSHOTS;
use crate::song::{effective_tracks, flatten_automation};
use crate::{DeviceKey, RackDoc};
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};

/// The param key of the snapshot lane.
pub const SNAPSHOT_LANE_KEY: &str = "snapshot";

/// What the snapshot lane is a lane of: the number of a snapshot, a whole number from 1 to
/// [`MAX_SNAPSHOTS`]. It has no param id (the master has no such param).
pub static SNAPSHOT_LANE_PARAM: ParamDescriptor = ParamDescriptor {
    id: ParamId(0),
    key: SNAPSHOT_LANE_KEY,
    name: "Snapshot recall",
    min: 1.0,
    max: MAX_SNAPSHOTS as f32,
    default: 1.0,
    taper: Taper::Linear,
    unit: Unit::None,
    discrete: true,
};

/// The descriptor of lane param `key` when it is the snapshot lane of a device of type
/// `type_id` (only the master has one).
pub(crate) fn lane_descriptor(type_id: &str, key: &str) -> Option<&'static ParamDescriptor> {
    (type_id == MASTER_TYPE && key == SNAPSHOT_LANE_KEY).then_some(&SNAPSHOT_LANE_PARAM)
}

/// The snapshot a lane point value names.
pub fn lane_snapshot(value: f32) -> u32 {
    if value.is_nan() {
        return 1;
    }
    value.round().clamp(1.0, MAX_SNAPSHOTS as f32) as u32
}

impl RackDoc {
    /// The master device and the index of the snapshot lane on its track, when the song has it.
    pub fn snapshot_lane(&self) -> Option<(DeviceKey, usize)> {
        let master = self.console.master.as_ref()?;
        let lane = self
            .song
            .track(master)?
            .automation_lane(SNAPSHOT_LANE_KEY)?;
        Some((master.clone(), lane))
    }

    /// The recalls the snapshot lane makes, as `(song tick, snapshot id)` in song order: one for
    /// each point whose snapshot differs from the point before it. Empty without the lane, for a
    /// muted lane or master track, and for a snapshot that does not exist (the point is kept
    /// in the song and plays nothing).
    pub fn snapshot_lane_events(&self) -> Vec<(i64, u32)> {
        let Some((master, lane)) = self.snapshot_lane() else {
            return Vec::new();
        };
        let Some(track) = effective_tracks(&self.song)
            .into_iter()
            .find(|t| t.device == master)
        else {
            return Vec::new();
        };
        let mut out: Vec<(i64, u32)> = Vec::new();
        let mut previous = None;
        for (tick, value, _) in flatten_automation(track, lane) {
            let id = lane_snapshot(value);
            if previous.replace(id) != Some(id) && self.console.snapshot(id).is_some() {
                out.push((tick, id));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_names_the_nearest_snapshot_within_range() {
        assert_eq!(lane_snapshot(1.0), 1);
        assert_eq!(lane_snapshot(2.4), 2);
        assert_eq!(lane_snapshot(2.6), 3);
        assert_eq!(lane_snapshot(0.0), 1);
        assert_eq!(lane_snapshot(-5.0), 1);
        assert_eq!(lane_snapshot(500.0), MAX_SNAPSHOTS as u32);
        assert_eq!(lane_snapshot(f32::NAN), 1);
    }

    #[test]
    fn only_the_master_has_the_lane() {
        assert!(lane_descriptor(MASTER_TYPE, "snapshot").is_some());
        assert!(lane_descriptor(MASTER_TYPE, "fader").is_none());
        assert!(lane_descriptor("atomik.channel", "snapshot").is_none());
    }
}
