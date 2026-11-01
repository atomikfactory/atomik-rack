//! The cursor of the rack and the arrangement while something is dragged: the open hand over a
//! device plate, the closed hand while a device moves, copy for a palette row dragged out,
//! not-allowed where a release drops nothing.

mod support;

use atomik_ui::kurbo::Point;
use atomik_ui::{CursorHint, RackView};
use support::*;

fn setup() -> (atomik_model::RackDoc, atomik_device_sdk::Registry, RackView) {
    let (doc, reg) = (reference_doc(), registry());
    let v = plain_view(&doc, &reg);
    (doc, reg, v)
}

#[test]
fn the_plate_of_a_device_shows_the_open_hand_and_the_body_the_arrow() {
    let (doc, reg, mut v) = setup();
    assert_eq!(v.cursor(), CursorHint::Default);
    let plate = v.plate_center(&key("delay")).unwrap();
    let _ = v.handle(&mv(plate), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Hand);
    let r = v.device_rect(&key("delay")).unwrap();
    let off = v.rack_to_view(Point::new(r.x1 + 5.0, r.y0 + 5.0));
    let _ = v.handle(&mv(off), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Default);
}

#[test]
fn a_device_moved_past_the_threshold_shows_the_closed_hand_until_it_leaves_the_rack() {
    let (doc, reg, mut v) = setup();
    let plate = v.plate_center(&key("delay")).unwrap();
    let _ = v.handle(&down(plate), &doc, &reg);
    // A press without travel is not a drag yet.
    assert_ne!(v.cursor(), CursorHint::Dragging);
    let synth = v.device_rect(&key("synth")).unwrap();
    let over_synth = v.rack_to_view(Point::new(synth.x0 + 300.0, synth.y0 + 40.0));
    let _ = v.handle(&mv(over_synth), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Dragging);
    // Pointer past the right edge of the rack area (over another pane): the drop is refused.
    let outside = Point::new(plate.x + 5_000.0, plate.y);
    let _ = v.handle(&mv(outside), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::NotAllowed);
    let _ = v.handle(&up(over_synth), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Default);
}
