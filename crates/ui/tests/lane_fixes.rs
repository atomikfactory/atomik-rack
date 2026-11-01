//! Three lane fixes of the arrangement: Delete on a picked lane removes the lane and not its
//! track, a click on empty automation lane draws a clip with a point (one undo step), and the
//! followed lane folds when its chevron is clicked.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{ClipContent, Command, Document, SongCommand};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::{ArrangementView, LaneButton, UiAction};
use support::song::*;
use support::*;

const CUTOFF: &str = "filter.cutoff";
const RESO: &str = "filter.resonance";

fn setup() -> (Document, Registry, ArrangementView) {
    let reg = registry();
    let mut d = Document::new(song_doc());
    d.apply(
        Command::Song(SongCommand::AddAutomationLane {
            device: key("bass"),
            param: RESO.into(),
        }),
        &reg,
    )
    .unwrap();
    let v = arrangement(d.doc(), &reg);
    (d, reg, v)
}

fn click(d: &Document, reg: &Registry, v: &mut ArrangementView, p: Point) -> Vec<UiAction> {
    run(|e| v.handle(e, d.doc(), reg), &[down(p), up(p)])
}

fn handled(outcome: CommandOutcome) -> Vec<UiAction> {
    match outcome {
        CommandOutcome::Handled(a) => a,
        other => panic!("not handled: {other:?}"),
    }
}

/// A point in the header of an automation lane, away from its buttons.
fn header_of(d: &Document, v: &ArrangementView, param: &str) -> Point {
    let lane = {
        let t = d.doc().song.track(&key("bass")).unwrap();
        t.automation_lane(param).unwrap()
    };
    let r = v.lane_rect(&key("bass"), lane).unwrap();
    Point::new(60.0, (r.y0 + r.y1) / 2.0)
}

#[test]
fn delete_on_a_picked_automation_lane_removes_only_the_lane() {
    let (d, reg, mut v) = setup();
    let p = header_of(&d, &v, RESO);
    let _ = click(&d, &reg, &mut v, p);
    assert_eq!(
        v.picked_automation_lane(),
        Some(&(key("bass"), RESO.to_owned()))
    );
    let a = handled(v.command("edit.delete", None, d.doc(), &reg));
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::RemoveAutomationLane {
            device: key("bass"),
            param: RESO.into(),
        }]
    );
    // One undo step: the lane is back, and the track was never touched.
    let mut d = d;
    for c in commands(&a) {
        d.apply(c, &reg).unwrap();
    }
    let t = d.doc().song.track(&key("bass")).unwrap();
    assert!(t.automation_lane(RESO).is_none());
    assert!(t.automation_lane(CUTOFF).is_some());
    d.undo(&reg).unwrap();
    assert!(
        d.doc()
            .song
            .track(&key("bass"))
            .unwrap()
            .automation_lane(RESO)
            .is_some()
    );
}

#[test]
fn delete_on_a_track_header_still_removes_the_track() {
    let (d, reg, mut v) = setup();
    let p = v.track_header_center(&key("bass")).unwrap();
    let _ = click(&d, &reg, &mut v, p);
    let a = handled(v.command("edit.delete", None, d.doc(), &reg));
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::RemoveTrack {
            device: key("bass")
        }]
    );
}

#[test]
fn delete_on_a_picked_extra_note_lane_removes_the_lane_not_the_track() {
    let (mut d, reg, _) = setup();
    d.apply(
        Command::Song(SongCommand::AddNoteLane {
            device: key("bass"),
            name: None,
        }),
        &reg,
    )
    .unwrap();
    let mut v = arrangement(d.doc(), &reg);
    let lane = d.doc().song.track(&key("bass")).unwrap().note_lane_count() - 1;
    let p = v.note_lane_header_center(&key("bass"), lane).unwrap();
    let _ = click(&d, &reg, &mut v, p);
    let a = handled(v.command("edit.delete", None, d.doc(), &reg));
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::RemoveNoteLane {
            device: key("bass"),
            lane
        }]
    );
}

#[test]
fn a_click_on_empty_automation_lane_draws_a_clip_with_a_point() {
    let (mut d, reg, mut v) = setup();
    let lane = d
        .doc()
        .song
        .track(&key("bass"))
        .unwrap()
        .automation_lane(RESO)
        .unwrap();
    let r = v.lane_rect(&key("bass"), lane).unwrap();
    // Inside the third bar, a beat in, near the top of the lane.
    let p = Point::new(v.tick_to_x(2 * BAR + PPQ), r.y0 + 10.0);
    let a = click(&d, &reg, &mut v, p);
    let songs = song_commands(&a);
    assert_eq!(songs.len(), 1, "one command, so one undo step: {songs:?}");
    let SongCommand::AddClip {
        device,
        lane: l,
        clip,
    } = &songs[0]
    else {
        panic!("{songs:?}")
    };
    assert_eq!((device, *l), (&key("bass"), lane));
    assert_eq!((clip.start, clip.length), (2 * BAR, BAR));
    let ClipContent::Automation(points) = &clip.content else {
        panic!("not automation")
    };
    assert_eq!(points.len(), 1);
    assert!((PPQ - points[0].tick).abs() <= PPQ / 2, "{points:?}");
    let before = d.doc().song.clone();
    for c in commands(&a) {
        d.apply(c, &reg).unwrap();
    }
    assert_eq!(
        d.doc().song.track(&key("bass")).unwrap().lanes[lane]
            .clips
            .len(),
        1
    );
    d.undo(&reg).unwrap();
    assert_eq!(*d.doc().song, *before);
}

#[test]
fn a_click_on_a_clip_body_inserts_a_point() {
    let (d, reg, mut v) = setup();
    let (top, bottom) = v.automation_value_range(atomik_model::ClipId(4)).unwrap();
    let p = Point::new(v.tick_to_x(BAR + PPQ), (top + bottom) / 2.0);
    let a = click(&d, &reg, &mut v, p);
    assert!(
        song_commands(&a)
            .iter()
            .any(|c| matches!(c, SongCommand::SetClipContent { .. })),
        "{a:?}"
    );
}

#[test]
fn the_followed_lane_folds_when_its_chevron_is_clicked() {
    let (d, reg, mut v) = setup();
    v.follow_lane(Some((key("bass"), RESO.to_owned())));
    v.sync(d.doc(), &reg);
    let lane = d
        .doc()
        .song
        .track(&key("bass"))
        .unwrap()
        .automation_lane(RESO)
        .unwrap();
    let open = v.lane_rect(&key("bass"), lane).unwrap().height();
    let p = v
        .lane_button_center(&key("bass"), RESO, LaneButton::Fold)
        .unwrap();
    let a = click(&d, &reg, &mut v, p);
    // The project did not fold it, so the click writes the fold.
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SetLaneFolded {
            device: key("bass"),
            param: RESO.into(),
            folded: true,
        }]
    );
    let mut d = d;
    for c in commands(&a) {
        d.apply(c, &reg).unwrap();
    }
    v.sync(d.doc(), &reg);
    let folded = v.lane_rect(&key("bass"), lane).unwrap().height();
    assert!(folded < open, "{folded} against {open}");
    // The same click unfolds it.
    let p = v
        .lane_button_center(&key("bass"), RESO, LaneButton::Fold)
        .unwrap();
    let a = click(&d, &reg, &mut v, p);
    for c in commands(&a) {
        d.apply(c, &reg).unwrap();
    }
    v.sync(d.doc(), &reg);
    assert_eq!(v.lane_rect(&key("bass"), lane).unwrap().height(), open);
}

#[test]
fn a_followed_lane_the_project_folds_folds_back_after_a_click_without_a_command() {
    let (mut d, reg, mut v) = setup();
    d.apply(
        Command::Song(SongCommand::SetLaneFolded {
            device: key("bass"),
            param: RESO.into(),
            folded: true,
        }),
        &reg,
    )
    .unwrap();
    v.follow_lane(Some((key("bass"), RESO.to_owned())));
    v.sync(d.doc(), &reg);
    let lane = d
        .doc()
        .song
        .track(&key("bass"))
        .unwrap()
        .automation_lane(RESO)
        .unwrap();
    // Forced open although the project folds it.
    let open = v.lane_rect(&key("bass"), lane).unwrap().height();
    assert!(open > 40.0);
    let p = v
        .lane_button_center(&key("bass"), RESO, LaneButton::Fold)
        .unwrap();
    let a = click(&d, &reg, &mut v, p);
    assert!(song_commands(&a).is_empty(), "{a:?}");
    v.sync(d.doc(), &reg);
    assert!(v.lane_rect(&key("bass"), lane).unwrap().height() < open);
    // Following another parameter lets the lane keep the project's fold; following this one
    // again forces it open again.
    v.follow_lane(Some((key("bass"), CUTOFF.to_owned())));
    v.follow_lane(Some((key("bass"), RESO.to_owned())));
    v.sync(d.doc(), &reg);
    assert_eq!(v.lane_rect(&key("bass"), lane).unwrap().height(), open);
}

#[test]
fn a_drag_on_an_empty_automation_lane_with_select_is_a_band_not_a_clip() {
    let (d, reg, mut v) = setup();
    let lane = d
        .doc()
        .song
        .track(&key("bass"))
        .unwrap()
        .automation_lane(RESO)
        .unwrap();
    let r = v.lane_rect(&key("bass"), lane).unwrap();
    let y = r.y0 + 10.0;
    let (a, b) = (v.tick_to_x(2 * BAR), v.tick_to_x(3 * BAR));
    let acts = run(
        |e| v.handle(e, d.doc(), &reg),
        &[
            down(Point::new(a, y)),
            mv(Point::new(b, y)),
            up(Point::new(b, y)),
        ],
    );
    assert!(commands(&acts).is_empty(), "{acts:?}");
}
