//! The arrangement's mouse cursor: the active tool's cursor over the lanes, the horizontal
//! resize arrows on a clip's edge (Select only, and during the drag), the arrow elsewhere.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::ClipId;
use atomik_model::RackDoc;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, CursorHint, Tool};
use support::song::*;
use support::*;

fn setup() -> (RackDoc, Registry, ArrangementView) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    (doc, reg, v)
}

fn hover(v: &mut ArrangementView, doc: &RackDoc, reg: &Registry, p: Point) -> CursorHint {
    v.handle(&mv(p), doc, reg);
    v.cursor()
}

/// Inside clip 1, clear of its edges and its header strip.
fn body(v: &ArrangementView) -> Point {
    let r = v.clip_rect(ClipId(1)).unwrap();
    Point::new(r.center().x, r.y1 - 6.0)
}

/// Just inside the right edge of clip 1.
fn edge(v: &ArrangementView) -> Point {
    let r = v.clip_rect(ClipId(1)).unwrap();
    Point::new(r.x1 - 2.0, r.center().y)
}

/// Empty lane space: right of every clip of the bass track.
fn empty_lane(v: &ArrangementView) -> Point {
    let r = v.lane_rect(&key("bass"), 0).unwrap();
    Point::new(r.x1 - 20.0, r.center().y)
}

#[test]
fn each_tool_shows_its_cursor_over_the_lanes_and_clips() {
    let (doc, reg, mut v) = setup();
    let (b, e) = (body(&v), empty_lane(&v));
    assert_eq!(hover(&mut v, &doc, &reg, b), CursorHint::Default);
    assert_eq!(hover(&mut v, &doc, &reg, e), CursorHint::Default);
    for (tool, hint) in [
        (Tool::Pencil, CursorHint::Pen),
        (Tool::Eraser, CursorHint::Eraser),
        (Tool::Razor, CursorHint::Razor),
    ] {
        v.set_tool(tool);
        assert_eq!(hover(&mut v, &doc, &reg, b), hint, "{tool:?} on a clip");
        assert_eq!(hover(&mut v, &doc, &reg, e), hint, "{tool:?} on a lane");
        // The clip edge does not resize with another tool than Select.
        let ed = edge(&v);
        assert_eq!(hover(&mut v, &doc, &reg, ed), hint, "{tool:?} on an edge");
    }
}

#[test]
fn headers_and_the_ruler_keep_the_arrow_whatever_the_tool() {
    let (doc, reg, mut v) = setup();
    let header = v.track_header_center(&key("bass")).unwrap();
    let ruler = Point::new(v.tick_to_x(3 * BAR + BAR / 2), v.ruler_label_y());
    for tool in Tool::ALL {
        v.set_tool(tool);
        assert_eq!(hover(&mut v, &doc, &reg, header), CursorHint::Default);
        assert_eq!(hover(&mut v, &doc, &reg, ruler), CursorHint::Default);
    }
}

#[test]
fn a_clip_edge_shows_the_resize_arrows_with_select_and_during_the_drag() {
    let (doc, reg, mut v) = setup();
    let ed = edge(&v);
    assert_eq!(hover(&mut v, &doc, &reg, ed), CursorHint::ResizeHorizontal);
    let b = body(&v);
    assert_eq!(hover(&mut v, &doc, &reg, b), CursorHint::Default);
    // The drag keeps the arrows wherever the pointer goes.
    v.handle(&down(ed), &doc, &reg);
    v.handle(&mv(ed + Vec2::new(30.0, 60.0)), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::ResizeHorizontal);
    v.handle(&up(ed), &doc, &reg);
    assert_eq!(
        v.cursor(),
        CursorHint::ResizeHorizontal,
        "still on the edge"
    );
    let away = empty_lane(&v);
    assert_eq!(hover(&mut v, &doc, &reg, away), CursorHint::Default);
}

#[test]
fn leaving_the_view_resets_the_cursor() {
    let (doc, reg, mut v) = setup();
    let ed = edge(&v);
    assert_eq!(hover(&mut v, &doc, &reg, ed), CursorHint::ResizeHorizontal);
    v.handle(&atomik_ui::UiEvent::PointerLeave, &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Default);
}
