//! What a drag of the selection offers to another song's window: the views say when they carry
//! a selection, end the drag without moving anything, and name where a drop lands.

mod support;

use atomik_model::{ClipId, RackSlot, SongCommand};
use atomik_ui::ArrangementView;
use atomik_ui::kurbo::{Point, Vec2};
use support::song::*;
use support::*;

fn setup() -> (
    ArrangementView,
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
) {
    let (doc, reg) = (song_doc(), registry());
    let v = arrangement(&doc, &reg);
    (v, doc, reg)
}

fn on_clip(v: &ArrangementView, id: u64) -> Point {
    let r = v.clip_rect(ClipId(id)).unwrap();
    Point::new(r.center().x, r.center().y + 6.0)
}

#[test]
fn a_clip_drag_carries_the_selection_until_it_is_released_or_cancelled() {
    let (mut v, doc, reg) = setup();
    assert!(!v.carries_selection());
    let from = on_clip(&v, 1);
    v.handle(&down(from), &doc, &reg);
    v.handle(&mv(from + Vec2::new(60.0, 0.0)), &doc, &reg);
    assert!(v.carries_selection());
    // Cancelling the drag puts back what it moved: the actions restore the first position.
    let actions = v.cancel_carried();
    assert!(!v.carries_selection());
    let starts: Vec<i64> = song_commands(&actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipGeometry { start, .. } => Some(start),
            _ => None,
        })
        .collect();
    let original = doc.song.clip(ClipId(1)).unwrap().2.start;
    assert!(starts.iter().all(|s| *s == original), "{starts:?}");
    // With no drag, cancelling does nothing.
    assert!(v.cancel_carried().is_empty());
}

#[test]
fn a_track_header_drag_is_a_carried_selection_too() {
    let (mut v, doc, reg) = setup();
    let header = v.track_header_center(&key("bass")).expect("a header");
    v.handle(&down(header), &doc, &reg);
    v.handle(&mv(header + Vec2::new(0.0, 30.0)), &doc, &reg);
    assert!(v.carries_selection());
    let actions = v.cancel_carried();
    assert!(
        !song_commands(&actions)
            .iter()
            .any(|c| matches!(c, SongCommand::MoveTracks { .. })),
        "{actions:?}"
    );
    assert!(!v.carries_selection());
}

#[test]
fn a_drop_spot_names_the_lane_under_the_pointer_and_the_snapped_tick() {
    let (v, ..) = setup();
    let lane = v.lane_rect(&key("bass"), 0).unwrap();
    let at = Point::new(v.tick_to_x(2 * BAR), lane.center().y);
    let (spot, tick) = v.drop_spot_at(at).expect("inside the track area");
    assert_eq!(spot, Some((key("bass"), 0)));
    assert_eq!(tick, 2 * BAR);
    // Above the lanes (the ruler) there is no spot.
    assert!(v.drop_spot_at(Point::new(300.0, 2.0)).is_none());
    // Outside the view there is none either.
    assert!(v.drop_spot_at(Point::new(5_000.0, 100.0)).is_none());
}

#[test]
fn a_rack_drop_slot_is_inside_the_rack_and_a_device_drag_is_carried() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = plain_view(&doc, &reg);
    assert!(!v.carries_devices());
    let r = v.device_rect(&key("delay")).unwrap();
    let on = v.rack_to_view(Point::new(r.x0 + 300.0, (r.y0 + r.y1) / 2.0));
    let (container, slot) = v.drop_slot_at(on, &doc).expect("over the rack");
    assert_eq!(container, None);
    let _: RackSlot = slot;
    // Outside the rack area (the far corner of the window) there is no slot.
    assert!(v.drop_slot_at(Point::new(-50.0, -50.0), &doc).is_none());

    let plate = v.plate_center(&key("mixer")).unwrap();
    v.handle(&down(plate), &doc, &reg);
    v.handle(&mv(plate + Vec2::new(0.0, 80.0)), &doc, &reg);
    assert!(v.carries_devices());
    v.drop_carried();
    assert!(!v.carries_devices());
}
