//! Note lanes: a track holds one or more note lanes, then its automation lanes.

use atomik_model::{Lane, LaneKind};

#[test]
fn lane_constructors_build_empty_lanes_of_each_kind() {
    for (lane, kind) in [
        (Lane::notes(), LaneKind::Notes),
        (Lane::audio(), LaneKind::Audio),
        (
            Lane::automation("filter.cutoff"),
            LaneKind::Automation("filter.cutoff".into()),
        ),
        (Lane::new(LaneKind::Notes), LaneKind::Notes),
    ] {
        assert_eq!(lane.kind, kind);
        assert!(lane.clips.is_empty() && lane.take_groups.is_empty());
    }
}
