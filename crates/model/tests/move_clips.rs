//! `WorkflowCommand::MoveClips`: clips to other tracks and ticks (a drag across lanes), the
//! copy variant, the lane rule, rejections and the drag-merge group.

mod common;

use atomik_model::{
    ClipId, ClipMove, Command, Document, ModelError, SongCommand, WorkflowCommand, empty_doc,
};
use common::audio::*;
use common::song::*;
use common::workflow::*;
use common::*;

/// Two synth tracks (a, b), two clips on a and an audio track.
fn arranged(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("a")),
            add(1, SYNTH, Some("b")),
            add_track("a"),
            add_track("b"),
            add_clip("a", 0, notes_clip(BAR, BAR, vec![note(0, PPQ, 60, 0.8)])),
            add_clip(
                "a",
                0,
                notes_clip(3 * BAR, BAR, vec![note(0, PPQ, 62, 0.8)]),
            ),
            add_audio_track(),
        ],
    );
    d
}

fn mv(id: u64, track: &str, start: i64) -> ClipMove {
    ClipMove {
        id: ClipId(id),
        track: key(track),
        start,
    }
}

fn move_cmd(moves: Vec<ClipMove>, copy: bool, merge: Option<u64>) -> Command {
    wf(WorkflowCommand::MoveClips { moves, copy, merge })
}

fn starts(d: &Document, track: &str) -> Vec<(u64, i64)> {
    d.doc().song.track(&key(track)).unwrap().lanes[0]
        .clips
        .iter()
        .map(|c| (c.id.0, c.start))
        .collect()
}

#[test]
fn moving_clips_to_another_track_keeps_ids_and_is_one_undo_step() {
    let r = audio_registry();
    let mut d = arranged(&r);
    let all = clip_ids(d.doc());
    let (x, y) = (all[0].0, all[1].0);
    one_step(
        &mut d,
        &r,
        move_cmd(vec![mv(x, "b", 2 * BAR), mv(y, "a", 5 * BAR)], false, None),
    );
    assert_eq!(starts(&d, "a"), [(y, 5 * BAR)]);
    assert_eq!(starts(&d, "b"), [(x, 2 * BAR)]);
    // Moving keeps the id counter.
    assert_eq!(d.doc().song.next_clip_id, all[1].0 + 1);
}

#[test]
fn copying_clips_leaves_the_originals_and_allocates_ids_in_order() {
    let r = audio_registry();
    let mut d = arranged(&r);
    let all = clip_ids(d.doc());
    let next = d.doc().song.next_clip_id;
    one_step(
        &mut d,
        &r,
        move_cmd(
            vec![mv(all[0].0, "b", BAR), mv(all[1].0, "b", 3 * BAR)],
            true,
            None,
        ),
    );
    assert_eq!(starts(&d, "a").len(), 2);
    assert_eq!(starts(&d, "b"), [(next, BAR), (next + 1, 3 * BAR)]);
    assert_eq!(d.last_clip_id(), Some(ClipId(next + 1)));
}

#[test]
fn a_track_without_the_lane_of_the_clip_rejects_the_whole_move() {
    let r = audio_registry();
    let mut d = arranged(&r);
    let audio_track = d
        .doc()
        .song
        .tracks
        .iter()
        .find(|t| t.is_audio())
        .unwrap()
        .device
        .0
        .clone();
    let all = clip_ids(d.doc());
    let e = rejected(
        &mut d,
        &r,
        move_cmd(
            vec![mv(all[0].0, "b", BAR), mv(all[1].0, &audio_track, BAR)],
            false,
            None,
        ),
    );
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    let e = rejected(
        &mut d,
        &r,
        move_cmd(vec![mv(all[0].0, &audio_track, BAR)], true, None),
    );
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn bad_input_is_rejected_without_a_change() {
    let r = audio_registry();
    let mut d = arranged(&r);
    let id = clip_ids(d.doc())[0].0;
    for moves in [
        Vec::new(),
        vec![mv(77, "b", BAR)],
        vec![mv(id, "nope", BAR)],
        vec![mv(id, "b", -1)],
        vec![mv(id, "b", BAR), mv(id, "b", 2 * BAR)],
    ] {
        rejected(&mut d, &r, move_cmd(moves, false, None));
    }
}

#[test]
fn a_cross_lane_drop_joins_the_geometry_drags_of_its_gesture() {
    let r = audio_registry();
    let mut d = arranged(&r);
    let all = clip_ids(d.doc());
    let before = text(&d);
    let geometry = |id: ClipId, start: i64| {
        sc(SongCommand::SetClipGeometry {
            id,
            start,
            length: BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            merge: Some(5),
        })
    };
    // Two clips dragged right twice (alternating commands), then dropped on track b.
    run(
        &mut d,
        &r,
        [
            geometry(all[0], 2 * BAR),
            geometry(all[1], 4 * BAR),
            geometry(all[0], 3 * BAR),
            geometry(all[1], 5 * BAR),
            move_cmd(
                vec![mv(all[0].0, "b", 3 * BAR), mv(all[1].0, "b", 5 * BAR)],
                false,
                Some(5),
            ),
        ],
    );
    assert_eq!(starts(&d, "b").len(), 2);
    d.end_merge();
    d.undo(&r).expect("one step");
    assert_eq!(text(&d), before, "the whole gesture is one undo step");
}

#[test]
fn a_group_drag_without_a_lane_change_is_one_undo_step_too() {
    let r = audio_registry();
    let mut d = arranged(&r);
    let all = clip_ids(d.doc());
    let before = text(&d);
    for dt in [BAR, 2 * BAR] {
        for (id, start) in [(all[0], BAR), (all[1], 3 * BAR)] {
            d.apply(
                sc(SongCommand::SetClipGeometry {
                    id,
                    start: start + dt,
                    length: BAR,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    merge: Some(9),
                }),
                &r,
            )
            .unwrap();
        }
    }
    d.end_merge();
    d.undo(&r).unwrap();
    assert_eq!(text(&d), before);
    d.redo(&r).unwrap();
    assert_eq!(starts(&d, "a"), [(all[0].0, 3 * BAR), (all[1].0, 5 * BAR)]);
}
