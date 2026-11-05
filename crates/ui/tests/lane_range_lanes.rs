//! A time range selected across several lanes (`Alt+Shift` drag that moves onto other lanes):
//! which lanes it covers, what Delete takes out of them, and that it stays one range.

mod support;

use atomik_model::{ClipId, WorkflowCommand};
use atomik_ui::keymap::{CommandOutcome, Os, Roles};
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, Modifiers};
use support::song::*;
use support::*;

fn setup() -> (
    ArrangementView,
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_snap(Snap::Bar);
    (v, doc, reg)
}

fn alt_shift() -> Modifiers {
    Modifiers {
        alt: true,
        shift: true,
        ..Modifiers::NONE
    }
}

fn down_alt_shift(pos: Point) -> atomik_ui::UiEvent {
    atomik_ui::UiEvent::PointerDown {
        pos,
        button: atomik_ui::PointerButton::Primary,
        modifiers: alt_shift(),
        clicks: 1,
    }
}

fn at(v: &ArrangementView, dev: &str, lane: usize, tick: i64) -> Point {
    let r = v.lane_rect(&key(dev), lane).unwrap();
    Point::new(v.tick_to_x(tick), r.center().y + 6.0)
}

fn drag(
    v: &mut ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    a: Point,
    b: Point,
) {
    let _ = run(
        |e| v.handle(e, doc, reg),
        &[down_alt_shift(a), mv_with(b, alt_shift()), up(b)],
    );
}

#[test]
fn a_drag_onto_another_lane_of_the_track_covers_both_lanes() {
    let (mut v, doc, reg) = setup();
    let (a, b) = (at(&v, "bass", 0, BAR), at(&v, "bass", 1, 3 * BAR));
    drag(&mut v, &doc, &reg, a, b);
    let (lanes, start, end) = v.lane_ranges().unwrap();
    assert_eq!(lanes, vec![(key("bass"), 0), (key("bass"), 1)]);
    assert_eq!((start, end), (BAR, 3 * BAR));
    // The single-lane accessor names the first lane.
    assert_eq!(v.lane_range(), Some((key("bass"), 0, BAR, 3 * BAR)));
    assert!(v.selected_clips().is_empty());
}

#[test]
fn a_drag_over_several_tracks_covers_every_lane_between_in_display_order() {
    let (mut v, doc, reg) = setup();
    // From the keys track up to the bass automation lane: the lead track lies between.
    let (a, b) = (at(&v, "keys", 0, 3 * BAR), at(&v, "bass", 1, BAR));
    drag(&mut v, &doc, &reg, a, b);
    let (lanes, start, end) = v.lane_ranges().unwrap();
    assert_eq!(
        lanes,
        vec![(key("bass"), 1), (key("lead"), 0), (key("keys"), 0)]
    );
    assert_eq!((start, end), (BAR, 3 * BAR));
}

#[test]
fn a_drag_that_stays_on_its_lane_is_a_single_lane_range_as_before() {
    let (mut v, doc, reg) = setup();
    let (a, b) = (at(&v, "keys", 0, BAR), at(&v, "keys", 0, 3 * BAR));
    drag(&mut v, &doc, &reg, a, b);
    let (lanes, ..) = v.lane_ranges().unwrap();
    assert_eq!(lanes, vec![(key("keys"), 0)]);
    assert_eq!(v.lane_range(), Some((key("keys"), 0, BAR, 3 * BAR)));
}

#[test]
fn delete_cuts_the_range_out_of_the_clips_of_every_lane() {
    let (mut v, doc, reg) = setup();
    let (a, b) = (at(&v, "bass", 0, BAR), at(&v, "bass", 1, 2 * BAR));
    drag(&mut v, &doc, &reg, a, b);
    let CommandOutcome::Handled(actions) = v.command("edit.delete", None, &doc, &reg) else {
        panic!("handled")
    };
    let cuts = workflow_commands(&actions);
    assert_eq!(cuts.len(), 1, "one command: one undo step");
    let WorkflowCommand::CutOut {
        ids,
        start,
        end,
        copy,
    } = &cuts[0]
    else {
        panic!("{cuts:?}")
    };
    let mut ids = ids.clone();
    ids.sort();
    // The bass clip and the cutoff clip lie inside bars 1..2; the looped clip starts later.
    assert_eq!(ids, vec![ClipId(1), ClipId(4)]);
    assert_eq!((*start, *end, *copy), (BAR, 2 * BAR, false));
    assert!(v.lane_ranges().is_none(), "the range is spent");
}

#[test]
fn a_range_over_lanes_can_be_set_and_forgotten_by_the_host() {
    let (mut v, _doc, _reg) = setup();
    v.set_lane_ranges(vec![(key("bass"), 0), (key("keys"), 0)], BAR, 2 * BAR);
    assert_eq!(v.lane_ranges().unwrap().0.len(), 2);
    // A lane that is gone drops out; the others stay.
    v.set_lane_ranges(vec![(key("bass"), 7), (key("keys"), 0)], BAR, 2 * BAR);
    assert_eq!(v.lane_ranges().unwrap().0, vec![(key("keys"), 0)]);
    v.set_lane_ranges(vec![(key("bass"), 7)], BAR, 2 * BAR);
    assert_eq!(v.lane_ranges(), None, "no lane left, no range");
    v.set_lane_ranges(Vec::new(), BAR, 2 * BAR);
    assert_eq!(v.lane_ranges(), None);
    v.set_lane_ranges(vec![(key("keys"), 0)], BAR, BAR);
    assert_eq!(v.lane_ranges(), None, "an empty span selects nothing");
    v.set_lane_range(key("keys"), 0, BAR, 2 * BAR);
    v.clear_lane_range();
    assert_eq!(v.lane_ranges(), None);
}
