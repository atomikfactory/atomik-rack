//! Pattern lanes in the arrangement: the lane is drawn as numbered blocks, a right click on a
//! clip opens the pattern picker (and Delete Clip), and a clip drawn on the lane starts with one
//! stepped point.

mod support;

use atomik_model::{AutomationPoint, ClipContent, ClipId, Curve, Lane, SongCommand};
use atomik_ui::kurbo::Point;
use atomik_ui::{ArrangementView, UiAction};
use support::song::*;
use support::*;

const CLIP: ClipId = ClipId(20);

fn view() -> (
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
    ArrangementView,
) {
    let (mut doc, reg) = (song_doc(), registry());
    doc.devices
        .push(device("seq", "atomik.step-sequencer", "Step Sequencer"));
    let clip = auto_clip(
        20,
        BAR,
        2 * BAR,
        vec![ap(0, 3.0, Curve::Step), ap(BAR, 5.0, Curve::Step)],
    );
    doc.song_mut().tracks.push(track(
        "seq",
        vec![Lane::automation("pattern").with_clips(vec![clip])],
    ));
    doc.song_mut().next_clip_id = 21;
    let mut v = arrangement(&doc, &reg);
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1200.0, 900.0));
    v.sync(&doc, &reg);
    (doc, reg, v)
}

fn clip_commands(actions: &[UiAction]) -> Vec<SongCommand> {
    song_commands(actions)
}

#[test]
fn a_right_click_opens_the_picker_at_the_block_under_the_pointer() {
    let (doc, reg, mut v) = view();
    let r = v.clip_rect(CLIP).unwrap();
    // The second block (pattern 5) starts one bar into the clip.
    let at = Point::new(v.tick_to_x(2 * BAR + BAR / 2), r.center().y + 6.0);
    let out = v.handle(&right_down(at), &doc, &reg);
    assert!(
        !out.iter()
            .any(|a| matches!(a, UiAction::ContextMenu { .. })),
        "the picker takes the place of the clip menu"
    );
    let items = v.param_menu_items();
    assert_eq!(items.len(), 9, "eight patterns and Delete Clip: {items:?}");
    assert_eq!(items[0], "1");
    assert_eq!(items[8], "delete");
    assert_eq!(v.menu_current(), Some(4), "pattern 5 is the block's number");
    // Row 2 sets the block's number to 2 and keeps the other block.
    let row = v.param_menu_row_center(1).unwrap();
    let out = v.handle(&down(row), &doc, &reg);
    let set = clip_commands(&out);
    let Some(SongCommand::SetClipContent {
        id: CLIP,
        content: ClipContent::Automation(points),
        ..
    }) = set.first()
    else {
        panic!("{set:?}");
    };
    let want: Vec<AutomationPoint> = vec![ap(0, 3.0, Curve::Step), ap(BAR, 2.0, Curve::Step)];
    assert_eq!(points, &want);
}

#[test]
fn the_picker_deletes_the_clip() {
    let (doc, reg, mut v) = view();
    let r = v.clip_rect(CLIP).unwrap();
    v.handle(
        &right_down(Point::new(
            v.tick_to_x(2 * BAR + BAR / 2),
            r.center().y + 6.0,
        )),
        &doc,
        &reg,
    );
    let last = v.param_menu_items().len() - 1;
    let row = v.param_menu_row_center(last).unwrap_or_else(|| {
        // The list scrolls: bring the last row into view.
        v.handle(
            &atomik_ui::UiEvent::Wheel {
                delta: atomik_ui::kurbo::Vec2::new(0.0, -20.0),
                pos: Point::new(v.tick_to_x(2 * BAR) + 20.0, r.center().y + 40.0),
                modifiers: atomik_ui::Modifiers::NONE,
            },
            &doc,
            &reg,
        );
        v.param_menu_row_center(last)
            .expect("visible after scrolling")
    });
    let out = v.handle(&down(row), &doc, &reg);
    assert_eq!(
        clip_commands(&out),
        vec![SongCommand::RemoveClip { id: CLIP }]
    );
}

#[test]
fn a_clip_drawn_on_the_lane_starts_with_a_stepped_point() {
    let (doc, reg, mut v) = view();
    v.set_tool(atomik_ui::Tool::Pencil);
    let y = v.lane_rect(&key("seq"), 0).unwrap().center().y;
    let from = Point::new(v.tick_to_x(3 * BAR + BAR / 4), y);
    let to = Point::new(v.tick_to_x(3 * BAR + BAR / 2), y);
    let mut out = v.handle(&down(from), &doc, &reg);
    out.extend(v.handle(&mv(to), &doc, &reg));
    out.extend(v.handle(&up(to), &doc, &reg));
    let added = clip_commands(&out);
    let Some(SongCommand::AddClip { clip, .. }) = added.first() else {
        panic!("{added:?}");
    };
    let ClipContent::Automation(points) = &clip.content else {
        panic!("automation clip");
    };
    assert_eq!(points.len(), 1, "{points:?}");
    assert_eq!((points[0].tick, points[0].curve), (0, Curve::Step));
    assert!((1.0..=8.0).contains(&points[0].value));
}

#[test]
fn the_blocks_paint_numbers_and_leave_other_lanes_alone() {
    let (doc, reg, mut v) = view();
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let texts = dl.texts();
    assert!(texts.contains(&"3"), "{texts:?}");
    assert!(texts.contains(&"5"), "{texts:?}");
}

// ---- The snapshot lane -------------------------------------------------------------------

/// A master with a snapshot lane holding snapshot 4 for a bar, then snapshot 9 (named "Chorus"
/// and "Verse" in the view, no snapshot 9).
fn snapshot_view() -> (
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
    ArrangementView,
) {
    let (mut doc, reg) = (song_doc(), registry());
    doc.devices
        .push(device("master", "atomik.master", "Master"));
    let clip = auto_clip(
        20,
        BAR,
        2 * BAR,
        vec![ap(0, 4.0, Curve::Step), ap(BAR, 9.0, Curve::Step)],
    );
    doc.song_mut().tracks.push(track(
        "master",
        vec![Lane::automation(atomik_model::SNAPSHOT_LANE_KEY).with_clips(vec![clip])],
    ));
    doc.song_mut().next_clip_id = 21;
    let mut v = arrangement(&doc, &reg);
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1200.0, 900.0));
    v.set_snapshot_names([(2, "Intro"), (4, "Chorus"), (7, "Bridge")].into_iter());
    v.sync(&doc, &reg);
    (doc, reg, v)
}

#[test]
fn the_snapshot_lane_draws_blocks_with_the_snapshot_names() {
    let (doc, reg, mut v) = snapshot_view();
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let texts = dl.texts();
    assert!(texts.contains(&"Chorus"), "{texts:?}");
    // A snapshot that is gone shows its number.
    assert!(texts.contains(&"9"), "{texts:?}");
}

#[test]
fn the_snapshot_picker_lists_the_snapshots_and_a_row_sets_the_block() {
    let (doc, reg, mut v) = snapshot_view();
    let r = v.clip_rect(CLIP).unwrap();
    let at = Point::new(v.tick_to_x(BAR + BAR / 2), r.center().y + 6.0);
    v.handle(&right_down(at), &doc, &reg);
    let items = v.param_menu_items();
    assert_eq!(items, ["2", "4", "7", "delete"], "one row per snapshot");
    assert_eq!(v.menu_current(), Some(1), "snapshot 4 is the block's");
    let row = v.param_menu_row_center(2).unwrap();
    let out = v.handle(&down(row), &doc, &reg);
    let set = clip_commands(&out);
    let Some(SongCommand::SetClipContent {
        id: CLIP,
        content: ClipContent::Automation(points),
        ..
    }) = set.first()
    else {
        panic!("{set:?}");
    };
    let want: Vec<AutomationPoint> = vec![ap(0, 7.0, Curve::Step), ap(BAR, 9.0, Curve::Step)];
    assert_eq!(points, &want);
}

#[test]
fn a_clip_drawn_on_the_snapshot_lane_starts_with_the_first_snapshot() {
    let (doc, reg, mut v) = snapshot_view();
    v.set_tool(atomik_ui::Tool::Pencil);
    let y = v.lane_rect(&key("master"), 0).unwrap().center().y;
    let from = Point::new(v.tick_to_x(3 * BAR + BAR / 4), y);
    let to = Point::new(v.tick_to_x(3 * BAR + BAR / 2), y);
    let mut out = v.handle(&down(from), &doc, &reg);
    out.extend(v.handle(&mv(to), &doc, &reg));
    out.extend(v.handle(&up(to), &doc, &reg));
    let added = clip_commands(&out);
    let Some(SongCommand::AddClip { clip, .. }) = added.first() else {
        panic!("{added:?}");
    };
    let ClipContent::Automation(points) = &clip.content else {
        panic!("automation clip");
    };
    assert_eq!(points.len(), 1, "{points:?}");
    assert_eq!(
        (points[0].tick, points[0].value, points[0].curve),
        (0, 2.0, Curve::Step)
    );
}

#[test]
fn a_rename_or_a_new_snapshot_repaints_the_lane() {
    let (doc, reg, mut v) = snapshot_view();
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(!v.needs_repaint(), "painted");
    v.set_snapshot_names([(2, "Intro"), (4, "Chorus"), (7, "Bridge")].into_iter());
    assert!(!v.needs_repaint(), "the same names change nothing");
    v.set_snapshot_names([(2, "Intro"), (4, "Hook"), (7, "Bridge")].into_iter());
    assert!(v.needs_repaint());
}
