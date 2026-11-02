//! Bending an automation segment: the fine-adjust role (Shift) held on a segment and dragged
//! bends it so the curve follows the pointer, one undo step; a Shift-click straightens a bent
//! segment; without Shift a press still adds a point; discrete params get Step points.

mod support;

use atomik_model::{AutomationPoint, ClipContent, ClipId, Curve, SongCommand};
use atomik_ui::kurbo::Point;
use atomik_ui::{ArrangementView, Modifiers, PointerButton, UiAction, UiEvent};
use support::song::*;
use support::*;

const CLIP: ClipId = ClipId(4);

fn points_of(actions: &[UiAction]) -> Option<Vec<AutomationPoint>> {
    song_commands(actions)
        .into_iter()
        .rev()
        .find_map(|c| match c {
            SongCommand::SetClipContent {
                id: CLIP,
                content: ClipContent::Automation(p),
                ..
            } => Some(p),
            _ => None,
        })
}

fn shift_down(pos: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::SHIFT,
        clicks: 1,
    }
}

fn view() -> (
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
    ArrangementView,
) {
    let (doc, reg) = (song_doc(), registry());
    let v = arrangement(&doc, &reg);
    (doc, reg, v)
}

/// Presses (Shift) in the middle of the first segment, at fraction `f` of the clip's value range
/// from the top, and drags to `g`; returns the points the drag wrote.
fn bend(f: f64, g: f64) -> Option<Vec<AutomationPoint>> {
    let (doc, reg, mut v) = view();
    let (top, bottom) = v.automation_value_range(CLIP).unwrap();
    let x = v.tick_to_x(2 * BAR / 2);
    let press = Point::new(x, top + (bottom - top) * f);
    let mut out = v.handle(&shift_down(press), &doc, &reg);
    let to = Point::new(x, top + (bottom - top) * g);
    out.extend(v.handle(&mv_with(to, Modifiers::SHIFT), &doc, &reg));
    let ended = v.handle(&up(to), &doc, &reg);
    assert_eq!(ended, vec![UiAction::EndMerge], "one undo step");
    points_of(&out)
}

#[test]
fn dragging_a_segment_below_the_line_bends_it_in_and_above_it_bends_it_out() {
    let low = bend(0.5, 0.9).expect("the drag wrote points");
    let high = bend(0.5, 0.05).expect("the drag wrote points");
    assert_eq!(low.len(), 4, "no point was added");
    let (Curve::Bend(a), Curve::Bend(b)) = (low[0].curve, high[0].curve) else {
        panic!("{:?} {:?}", low[0], high[0]);
    };
    assert!(a > 0, "below the straight line eases in: {a}");
    assert!(b < 0, "above it eases out: {b}");
    // The other segments and the values are untouched.
    assert_eq!(low[1..], song_points()[1..]);
}

fn song_points() -> Vec<AutomationPoint> {
    let (doc, _, _) = view();
    let (_, _, c) = doc.song.clip(CLIP).unwrap();
    let ClipContent::Automation(p) = &c.content else {
        panic!("automation")
    };
    p.clone()
}

#[test]
fn a_click_without_a_drag_straightens_a_bent_segment_and_a_plain_press_adds_a_point() {
    let (mut doc, reg, _) = view();
    // The first segment bent.
    let c = doc.song.clip(CLIP).unwrap().2.clone();
    let ClipContent::Automation(mut pts) = c.content.clone() else {
        panic!("automation")
    };
    pts[0].curve = Curve::Bend(30);
    let song = doc.song_mut();
    for t in &mut song.tracks {
        for l in &mut t.lanes {
            for cl in &mut l.clips {
                if cl.id == CLIP {
                    cl.content = ClipContent::Automation(pts.clone());
                }
            }
        }
    }
    let mut v = arrangement(&doc, &reg);
    let (top, bottom) = v.automation_value_range(CLIP).unwrap();
    let p = Point::new(v.tick_to_x(BAR), (top + bottom) / 2.0);
    let mut out = v.handle(&shift_down(p), &doc, &reg);
    out.extend(v.handle(&up(p), &doc, &reg));
    let straight = points_of(&out).expect("the click straightened the segment");
    assert_eq!(straight[0].curve, Curve::Linear);
    assert_eq!(straight.len(), 4);
    // Without Shift the same press adds a point.
    let mut v = arrangement(&doc, &reg);
    let out = v.handle(&down(p), &doc, &reg);
    assert_eq!(points_of(&out).unwrap().len(), 5);
}

#[test]
fn a_shift_click_on_a_point_straightens_a_bent_segment_before_it_toggles_step() {
    let points = |curve| {
        vec![
            AutomationPoint {
                tick: 0,
                value: 0.0,
                curve,
            },
            AutomationPoint {
                tick: 100,
                value: 1.0,
                curve: Curve::Step,
            },
        ]
    };
    use atomik_ui::seq_edit::toggle_curve;
    assert_eq!(
        toggle_curve(&points(Curve::Bend(12)), 0)[0].curve,
        Curve::Linear
    );
    assert_eq!(
        toggle_curve(&points(Curve::Linear), 0)[0].curve,
        Curve::Step
    );
    assert_eq!(
        toggle_curve(&points(Curve::Step), 0)[0].curve,
        Curve::Linear
    );
}
