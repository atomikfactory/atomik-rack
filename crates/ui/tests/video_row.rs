//! The video row of the arrangement: it exists only for a song with a film, sits above the track
//! lanes and pushes them down, collapses and hides, lays thumbnails out per zoom and asks only for
//! the frames in view, draws its states (missing, no preview, pictures), moves and trims the film
//! through undoable commands with frame and grid snap, is selected and removed with Delete, and
//! never takes a press that belongs to a track.

mod support;

use atomik_model::{Command, RackDoc, VideoClip, VideoCommand, VideoFacts};
use atomik_ui::arrangement::{VIDEO_ROW_COLLAPSED, VIDEO_ROW_HEIGHT, VideoFeed};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, DisplayList, Key, UiAction};
use std::sync::Arc;
use support::song::*;
use support::*;

const FRAME_US: i64 = 40_000;

/// The disable-snap role: Ctrl and Alt together.
const NO_SNAP: atomik_ui::Modifiers = atomik_ui::Modifiers {
    shift: false,
    ctrl: true,
    alt: true,
    meta: false,
};

fn facts() -> VideoFacts {
    VideoFacts {
        duration_us: 60_000_000,
        fps_num: 25,
        fps_den: 1,
        width: 640,
        height: 360,
        timecode: None,
        has_audio: true,
    }
}

fn with_video() -> RackDoc {
    let mut doc = song_doc();
    doc.video = Some(VideoClip::new("films/take-one.mp4", facts()));
    doc
}

fn ready(frames: u32) -> VideoFeed {
    VideoFeed {
        preview_frames: Some(frames),
        playhead_timecode: "00:00:00:00".into(),
        start_timecode: "00:00:00:00".into(),
        end_timecode: "00:01:00:00".into(),
        ..VideoFeed::default()
    }
}

fn view_of(doc: &RackDoc) -> ArrangementView {
    let reg = registry();
    let mut v = arrangement(doc, &reg);
    v.set_video_feed(ready(1500));
    v
}

fn texts(v: &mut ArrangementView, doc: &RackDoc) -> Vec<String> {
    let reg = registry();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn video_commands(actions: &[UiAction]) -> Vec<VideoCommand> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Video(v) => Some(v),
            _ => None,
        })
        .collect()
}

fn thumb(v: &mut ArrangementView, frame: u32) {
    v.add_video_thumb(frame, 4, 2, Arc::new(vec![200; 4 * 2 * 4]));
}

#[test]
fn there_is_no_row_without_a_film_and_the_lanes_move_down_with_one() {
    let (plain, reg) = (song_doc(), registry());
    let mut v = arrangement(&plain, &reg);
    assert_eq!(v.video_row_rect().height(), 0.0);
    assert!(v.video_bar_rect().is_none());
    let before = v.lane_rect(&key("bass"), 0).unwrap();
    let doc = with_video();
    v.sync(&doc, &reg);
    assert_eq!(v.video_row_rect().height(), VIDEO_ROW_HEIGHT);
    let after = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((after.y0 - before.y0 - VIDEO_ROW_HEIGHT).abs() < 1e-9);
    // It sits right above the lanes.
    assert!((v.video_row_rect().y1 - after.y0).abs() < 1e-9);
}

#[test]
fn the_row_collapses_and_hides() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    let open = v.lane_rect(&key("bass"), 0).unwrap().y0;
    v.set_video_row_collapsed(true);
    assert!(v.video_row_collapsed());
    assert_eq!(v.video_row_rect().height(), VIDEO_ROW_COLLAPSED);
    assert!(
        v.video_slots().is_empty(),
        "a collapsed row has no pictures"
    );
    let thin = v.lane_rect(&key("bass"), 0).unwrap().y0;
    assert!((open - thin - (VIDEO_ROW_HEIGHT - VIDEO_ROW_COLLAPSED)).abs() < 1e-9);
    v.set_video_row(false);
    assert!(!v.video_row_visible());
    assert_eq!(v.video_row_rect().height(), 0.0);
    let hidden = v.lane_rect(&key("bass"), 0).unwrap().y0;
    assert!((open - hidden - VIDEO_ROW_HEIGHT).abs() < 1e-9);
    v.set_video_row(true);
    v.set_video_row_collapsed(false);
    let back = v.lane_rect(&key("bass"), 0).unwrap().y0;
    assert!((back - open).abs() < 1e-9);
    // The chevron of the header cell collapses it with a click.
    let row = v.video_row_rect();
    let chevron = Point::new(row.x0 + 12.0, row.y0 + 10.0);
    let actions = run(|e| v.handle(e, &doc, &reg), &[down(chevron), up(chevron)]);
    assert!(video_commands(&actions).is_empty());
    assert!(v.video_row_collapsed());
}

#[test]
fn slots_are_as_wide_as_a_picture_and_follow_the_zoom() {
    let doc = with_video();
    let mut v = view_of(&doc);
    let slots = v.video_slots();
    assert!(slots.len() > 3, "{}", slots.len());
    let w = slots[0].x1 - slots[0].x0;
    // 16:9 pictures as high as the bar: about 82 px wide.
    assert!(
        (w - (VIDEO_ROW_HEIGHT - 8.0) * 640.0 / 360.0).abs() < 1e-6,
        "{w}"
    );
    for pair in slots.windows(2) {
        assert!((pair[1].x0 - pair[0].x1).abs() < 1e-9, "slots touch");
        assert!(pair[1].frame >= pair[0].frame);
    }
    assert_eq!(slots[0].frame, 0, "the first slot shows the first frame");
    let bar = v.video_bar_rect().unwrap();
    assert!(
        (slots[0].x0 - bar.x0).abs() < 1e-9,
        "anchored to the film's start"
    );
    // Zooming in: the same slots now cover less time, so their frames are closer together.
    let spread = |s: &[atomik_ui::arrangement::VideoSlot]| s[s.len() - 1].frame - s[0].frame;
    let wide = spread(&slots);
    let ppt = v.px_per_tick();
    v.set_px_per_tick(ppt * 4.0);
    let zoomed = v.video_slots();
    assert!(!zoomed.is_empty());
    assert!(spread(&zoomed) < wide, "{} < {wide}", spread(&zoomed));
    assert!(
        (zoomed[0].x1 - zoomed[0].x0 - w).abs() < 1e-6,
        "same slot width"
    );
    // Zooming out far: the whole film fits in view, so slots stop at the end of the bar.
    v.set_px_per_tick(ppt / 40.0);
    let far = v.video_slots();
    let last = far.last().unwrap();
    let bar = v.video_bar_rect().unwrap();
    assert!(last.x0 < bar.x1 && last.x1 + w > bar.x1);
    assert!(far.iter().all(|s| s.frame < 1500));
}

#[test]
fn a_slot_shows_the_frame_that_plays_at_its_start_after_a_trim() {
    let mut doc = with_video();
    // Trimmed: starts 10 s into the film, plays 30 s of it, on the song's start.
    let clip = doc.video.as_mut().unwrap();
    clip.in_us = 10_000_000;
    clip.out_us = 40_000_000;
    let v = view_of(&doc);
    assert_eq!(v.video_slots()[0].frame, 250);
    // Frames never pass the preview's last one.
    let mut short = view_of(&doc);
    short.set_video_feed(ready(100));
    assert!(short.video_slots().iter().all(|s| s.frame <= 99));
}

#[test]
fn only_the_frames_in_view_are_asked_for_and_pictures_are_kept_by_frame() {
    let doc = with_video();
    let mut v = view_of(&doc);
    let slots = v.video_slots();
    let wanted = v.video_wanted();
    let mut frames: Vec<u32> = slots.iter().map(|s| s.frame).collect();
    frames.sort_unstable();
    frames.dedup();
    let mut asked = wanted.clone();
    asked.sort_unstable();
    assert_eq!(asked, frames, "every visible slot, nothing else");
    // Nearest the middle first.
    let middle = slots.iter().min_by(|a, b| {
        let d = |s: &atomik_ui::arrangement::VideoSlot| ((s.x0 + s.x1) / 2.0 - v_center(&v)).abs();
        d(a).total_cmp(&d(b))
    });
    assert_eq!(Some(wanted[0]), middle.map(|s| s.frame));
    // A decoded frame is no longer wanted.
    thumb(&mut v, wanted[0]);
    assert!(v.video_has_thumb(wanted[0]));
    assert!(!v.video_wanted().contains(&wanted[0]));
    // Not asked for while there is nothing to take pictures from.
    let mut none = view_of(&doc);
    none.set_video_feed(VideoFeed {
        preview_frames: None,
        ..ready(0)
    });
    assert!(none.video_wanted().is_empty());
    let mut missing = view_of(&doc);
    missing.set_video_feed(VideoFeed {
        missing: true,
        ..ready(1500)
    });
    assert!(missing.video_wanted().is_empty());
    v.clear_video_thumbs();
    assert_eq!(v.video_thumbs(), (0, 0));
}

fn v_center(v: &ArrangementView) -> f64 {
    let tl = v.video_row_rect();
    // The timeline starts at the right edge of the header cell, the same x the lanes use.
    let lane = v.lane_rect(&key("bass"), 0).unwrap();
    (lane.x0 + tl.x1) / 2.0
}

#[test]
fn the_thumbnails_stay_under_their_budget() {
    let doc = with_video();
    let mut v = view_of(&doc);
    // 1500 pictures of 256 by 144 would be 220 MB.
    for f in 0..1500 {
        v.add_video_thumb(f, 256, 144, Arc::new(vec![0; 256 * 144 * 4]));
    }
    let (count, bytes) = v.video_thumbs();
    assert!(bytes <= 10 * 1024 * 1024 + 256 * 144 * 4, "{bytes}");
    assert!(count < 1500 && count > 20, "{count}");
    assert!(v.video_has_thumb(1499), "the newest stays");
    // A picture that is too short for its size is refused.
    v.clear_video_thumbs();
    v.add_video_thumb(1, 8, 8, Arc::new(vec![0; 10]));
    assert_eq!(v.video_thumbs().0, 0);
}

#[test]
fn the_row_paints_its_states() {
    let doc = with_video();
    let mut v = view_of(&doc);
    thumb(&mut v, 0);
    let t = texts(&mut v, &doc);
    assert!(t.iter().any(|s| s == "VIDEO"), "{t:?}");
    assert!(
        t.iter().any(|s| s == "00:00:00:00"),
        "playhead timecode: {t:?}"
    );
    assert!(t.iter().any(|s| s == "take-one.mp4  00:00:00:00"), "{t:?}");
    // No preview yet: the note says why.
    v.set_video_feed(VideoFeed {
        preview_frames: None,
        note: "Preparing 40%".into(),
        ..ready(0)
    });
    let t = texts(&mut v, &doc);
    assert!(
        t.iter().any(|s| s == "take-one.mp4  Preparing 40%"),
        "{t:?}"
    );
    // Missing file.
    v.set_video_feed(VideoFeed {
        missing: true,
        ..ready(0)
    });
    let t = texts(&mut v, &doc);
    assert!(t.iter().any(|s| s == "MISSING  take-one.mp4"), "{t:?}");
    // Hidden: nothing of it is drawn.
    v.set_video_row(false);
    let t = texts(&mut v, &doc);
    assert!(!t.iter().any(|s| s == "VIDEO"));
}

#[test]
fn a_picture_is_painted_into_its_slot() {
    let doc = with_video();
    let reg = registry();
    let mut with_pictures = view_of(&doc);
    let mut without = view_of(&doc);
    for f in with_pictures.video_wanted() {
        thumb(&mut with_pictures, f);
    }
    let paint = |v: &mut ArrangementView| {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl
    };
    let (a, b) = (paint(&mut with_pictures), paint(&mut without));
    assert_ne!(
        format!("{a:?}"),
        format!("{b:?}"),
        "pictures change what is painted"
    );
}

/// The point on the bar, `dx` right of its left end, in the middle of its height.
fn on_bar(v: &ArrangementView, dx: f64) -> Point {
    let bar = v.video_bar_rect().unwrap();
    Point::new(bar.x0 + dx, bar.center().y)
}

#[test]
fn dragging_the_body_moves_the_film_by_whole_frames_in_one_undo_step() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    let from = on_bar(&v, 200.0);
    let to = Point::new(from.x + 137.0, from.y);
    let mid = Point::new(from.x + 60.0, from.y);
    let actions = run(
        |e| v.handle(e, &doc, &reg),
        &[down(from), mv(mid), mv(to), up(to)],
    );
    let moves = video_commands(&actions);
    assert!(moves.len() >= 2, "{moves:?}");
    let mut merges = Vec::new();
    for m in &moves {
        let VideoCommand::Move { start_us, merge } = m else {
            panic!("{m:?}")
        };
        assert!(*start_us > 0);
        assert_eq!(start_us % FRAME_US, 0, "a whole frame: {start_us}");
        merges.push(*merge);
    }
    assert!(merges[0].is_some() && merges.iter().all(|m| *m == merges[0]));
    assert!(matches!(actions.last(), Some(UiAction::EndMerge)));
    // The selection stays on the film.
    assert!(v.video_selected());
}

#[test]
fn the_grid_decides_first_and_the_frame_snap_follows() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    v.set_snap(Snap::Bar);
    // A bar is 2 s at 120 BPM: 50 whole frames, so a start on the bar is on a frame too.
    let bar_px = 4.0 * 15_360.0 * v.px_per_tick();
    let from = on_bar(&v, 100.0);
    let to = Point::new(from.x + bar_px * 1.02, from.y);
    let actions = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(from),
            mv(Point::new(from.x + 30.0, from.y)),
            mv(to),
            up(to),
        ],
    );
    let VideoCommand::Move { start_us, .. } = video_commands(&actions).pop().unwrap() else {
        panic!("a move")
    };
    assert_eq!(start_us, 2_000_000);
    // With a beat grid (0.5 s = 12.5 frames) the start is the frame next to the beat.
    let mut v = view_of(&doc);
    v.set_snap(Snap::Beat);
    let beat_px = 15_360.0 * v.px_per_tick();
    let from = on_bar(&v, 100.0);
    let to = Point::new(from.x + beat_px * 1.0, from.y);
    let actions = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(from),
            mv(Point::new(from.x + 30.0, from.y)),
            mv(to),
            up(to),
        ],
    );
    let VideoCommand::Move { start_us, .. } = video_commands(&actions).pop().unwrap() else {
        panic!("a move")
    };
    assert_eq!(start_us % FRAME_US, 0);
    assert!((start_us - 500_000).abs() <= FRAME_US / 2, "{start_us}");
}

#[test]
fn the_no_snap_modifier_moves_without_either_snap() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    let from = on_bar(&v, 200.0);
    let to = Point::new(from.x + 41.0, from.y);
    let actions = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(from),
            mv_with(Point::new(from.x + 10.0, from.y), NO_SNAP),
            mv_with(to, NO_SNAP),
            up(to),
        ],
    );
    let VideoCommand::Move { start_us, .. } = video_commands(&actions).pop().unwrap() else {
        panic!("a move")
    };
    assert!(start_us > 0 && start_us % FRAME_US != 0, "{start_us}");
}

#[test]
fn a_click_selects_without_a_command_and_escape_lets_go() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    let p = on_bar(&v, 150.0);
    let actions = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert!(video_commands(&actions).is_empty(), "{actions:?}");
    assert!(v.video_selected());
    let _ = run(
        |e| v.handle(e, &doc, &reg),
        &[press(Key::Escape, atomik_ui::Modifiers::NONE)],
    );
    assert!(!v.video_selected());
}

#[test]
fn the_left_end_trims_the_start_on_a_whole_source_frame_and_keeps_the_out_point() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    let from = on_bar(&v, 2.0);
    let to = Point::new(from.x + 211.0, from.y);
    let actions = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(from),
            mv(Point::new(from.x + 40.0, from.y)),
            mv(to),
            up(to),
        ],
    );
    let trims = video_commands(&actions);
    assert!(!trims.is_empty());
    let VideoCommand::Trim {
        in_us,
        out_us,
        merge,
    } = trims.last().unwrap()
    else {
        panic!("{trims:?}")
    };
    assert!(*in_us > 0 && in_us % FRAME_US as u64 == 0, "{in_us}");
    assert_eq!(*out_us, 60_000_000);
    assert!(merge.is_some());
    assert!(matches!(actions.last(), Some(UiAction::EndMerge)));
}

#[test]
fn the_right_end_trims_the_end_and_keeps_the_in_point() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    // Zoom out so the whole film's end is in view.
    let ppt = v.px_per_tick();
    v.set_px_per_tick(ppt / 8.0);
    let bar = v.video_bar_rect().unwrap();
    assert!(bar.x1 < 1190.0, "the end is in view: {}", bar.x1);
    let from = Point::new(bar.x1 - 2.0, bar.center().y);
    let to = Point::new(from.x - 150.0, from.y);
    let actions = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(from),
            mv(Point::new(from.x - 30.0, from.y)),
            mv(to),
            up(to),
        ],
    );
    let VideoCommand::Trim { in_us, out_us, .. } = video_commands(&actions).pop().unwrap() else {
        panic!("a trim")
    };
    assert_eq!(in_us, 0);
    assert!(
        out_us < 60_000_000 && out_us % FRAME_US as u64 == 0,
        "{out_us}"
    );
}

#[test]
fn escape_during_a_drag_puts_the_film_back_in_the_same_undo_step() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    let from = on_bar(&v, 200.0);
    let to = Point::new(from.x + 90.0, from.y);
    let moving = run(
        |e| v.handle(e, &doc, &reg),
        &[down(from), mv(Point::new(from.x + 20.0, from.y)), mv(to)],
    );
    let merge = match video_commands(&moving).last() {
        Some(VideoCommand::Move { merge, .. }) => *merge,
        other => panic!("{other:?}"),
    };
    let cancel = run(
        |e| v.handle(e, &doc, &reg),
        &[press(Key::Escape, atomik_ui::Modifiers::NONE)],
    );
    assert_eq!(
        video_commands(&cancel),
        [VideoCommand::Move { start_us: 0, merge }]
    );
    assert!(matches!(cancel.last(), Some(UiAction::EndMerge)));
    // The release afterwards does nothing more.
    let after = run(|e| v.handle(e, &doc, &reg), &[up(to)]);
    assert!(video_commands(&after).is_empty());
}

#[test]
fn delete_removes_a_selected_film_and_only_then() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    // Nothing selected: the command is not the row's.
    let outcome = v.command("edit.delete", None, &doc, &reg);
    assert!(
        !matches!(&outcome, CommandOutcome::Handled(a) if !video_commands(a).is_empty()),
        "nothing to remove yet"
    );
    let p = on_bar(&v, 150.0);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    let CommandOutcome::Handled(actions) = v.command("edit.delete", None, &doc, &reg) else {
        panic!("the selected film goes")
    };
    assert_eq!(video_commands(&actions), [VideoCommand::Remove]);
    assert!(!v.video_selected());
}

#[test]
fn a_press_on_a_track_never_selects_the_film_and_takes_the_selection_away() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    // A press in the middle of a track lane lies under the row's band: not in it.
    let lane = v.lane_rect(&key("bass"), 0).unwrap();
    let in_lane = Point::new(lane.x0 + 300.0, lane.center().y);
    assert!(in_lane.y > v.video_row_rect().y1);
    let actions = run(|e| v.handle(e, &doc, &reg), &[down(in_lane), up(in_lane)]);
    assert!(video_commands(&actions).is_empty());
    assert!(!v.video_selected());
    // With the film selected, a press on a lane lets go of it.
    let p = on_bar(&v, 150.0);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert!(v.video_selected());
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(in_lane), up(in_lane)]);
    assert!(!v.video_selected());
    // And the band's own edge: the last pixel of the row is the row, the first of the lane is the lane.
    let row = v.video_row_rect();
    let below = Point::new(lane.x0 + 300.0, row.y1 + 0.5);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(below), up(below)]);
    assert!(!v.video_selected());
}

#[test]
fn a_press_beside_the_bar_or_over_the_ruler_leaves_the_film_alone() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    let row = v.video_row_rect();
    let bar = v.video_bar_rect().unwrap();
    // Past the film's end (zoomed out so the end is in view).
    let ppt = v.px_per_tick();
    v.set_px_per_tick(ppt / 8.0);
    let bar_now = v.video_bar_rect().unwrap();
    assert!(bar_now.x1 < bar.x1);
    let past = Point::new(bar_now.x1 + 20.0, row.center().y);
    let actions = run(|e| v.handle(e, &doc, &reg), &[down(past), up(past)]);
    assert!(video_commands(&actions).is_empty());
    assert!(!v.video_selected());
    // The ruler above the row is the ruler's.
    let ruler = Point::new(bar_now.x0 + 100.0, 5.0);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(ruler), up(ruler)]);
    assert!(!v.video_selected());
}

#[test]
fn a_missing_or_unprepared_film_still_moves() {
    let (doc, reg) = (with_video(), registry());
    for feed in [
        VideoFeed {
            missing: true,
            ..ready(0)
        },
        VideoFeed {
            preview_frames: None,
            note: "No preview".into(),
            ..ready(0)
        },
    ] {
        let mut v = view_of(&doc);
        v.set_video_feed(feed);
        assert!(v.video_slots().is_empty());
        let from = on_bar(&v, 200.0);
        let to = Point::new(from.x + 100.0, from.y);
        let actions = run(
            |e| v.handle(e, &doc, &reg),
            &[
                down(from),
                mv(Point::new(from.x + 20.0, from.y)),
                mv(to),
                up(to),
            ],
        );
        assert!(
            video_commands(&actions)
                .iter()
                .any(|c| matches!(c, VideoCommand::Move { .. })),
            "{actions:?}"
        );
    }
}

#[test]
fn a_collapsed_row_still_edits() {
    let (doc, reg) = (with_video(), registry());
    let mut v = view_of(&doc);
    v.set_video_row_collapsed(true);
    let from = on_bar(&v, 200.0);
    let to = Point::new(from.x + 100.0, from.y);
    let actions = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(from),
            mv(Point::new(from.x + 20.0, from.y)),
            mv(to),
            up(to),
        ],
    );
    assert!(
        video_commands(&actions)
            .iter()
            .any(|c| matches!(c, VideoCommand::Move { .. }))
    );
}
