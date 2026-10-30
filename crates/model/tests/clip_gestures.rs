//! The clip gesture commands of the arrangement: left-edge trim, delete outside events, split
//! several clips at once and the razor cut-out (model half).

mod common;

use atomik_model::song::flatten_notes;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ControllerKind, ControllerLane, Curve, Document,
    SongCommand, SyncOp, WorkflowCommand, empty_doc,
};
use common::song::*;
use common::workflow::*;
use common::*;
use proptest::prelude::*;

fn doc_with(clips: Vec<(usize, Clip)>) -> (Document, atomik_device_sdk::Registry) {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
        ],
    );
    for (lane, c) in clips {
        run(&mut d, &r, [add_clip("synth", lane, c)]);
    }
    (d, r)
}

fn clip_of(d: &Document, id: u64) -> Clip {
    d.doc().song.clip(ClipId(id)).unwrap().2.clone()
}

fn notes_of(c: &Clip) -> Vec<(i64, i64, u8)> {
    let ClipContent::Notes(n) = &c.content else {
        panic!("notes")
    };
    n.iter().map(|n| (n.tick, n.length, n.key)).collect()
}

fn flat(d: &Document) -> Vec<(i64, i64, u8, f32)> {
    flatten_notes(d.doc().song.track(&key("synth")).unwrap())
}

fn trim(ids: &[u64], delta: i64) -> Command {
    wf(WorkflowCommand::TrimClips {
        ids: ids.iter().copied().map(ClipId).collect(),
        start_delta: delta,
        merge: None,
    })
}

// ---------------------------------------------------------------- trim

#[test]
fn trimming_the_start_moves_the_offset_and_keeps_the_end() {
    let (mut d, r) = doc_with(vec![(
        0,
        notes_clip(
            BAR,
            2 * BAR,
            vec![note(0, PPQ, 60, 0.8), note(BAR, PPQ, 62, 0.8)],
        ),
    )]);
    one_step(&mut d, &r, trim(&[1], PPQ));
    let c = clip_of(&d, 1);
    assert_eq!(
        (c.start, c.length, c.offset),
        (BAR + PPQ, 2 * BAR - PPQ, PPQ)
    );
    assert_eq!(c.end(), 3 * BAR);
    // The note before the new window no longer plays; the other keeps its song tick.
    let got: Vec<i64> = flat(&d).iter().map(|n| n.0).collect();
    assert_eq!(got, [2 * BAR]);
}

#[test]
fn growing_past_content_zero_shifts_notes_controllers_and_keeps_song_time() {
    let mut c = notes_clip(
        BAR,
        BAR,
        vec![note(0, PPQ, 60, 0.8), note(PPQ, PPQ, 64, 0.8)],
    );
    c.controllers = vec![ControllerLane::new(
        ControllerKind::PitchBend,
        vec![point(0, 0.0, Curve::Step), point(PPQ, 0.5, Curve::Step)],
    )];
    let (mut d, r) = doc_with(vec![(0, c)]);
    let before = flat(&d);
    one_step(&mut d, &r, trim(&[1], -PPQ));
    let c = clip_of(&d, 1);
    assert_eq!((c.start, c.length, c.offset), (BAR - PPQ, BAR + PPQ, 0));
    assert_eq!(notes_of(&c), [(PPQ, PPQ, 60), (2 * PPQ, PPQ, 64)]);
    assert_eq!(c.controllers[0].points[1].tick, 2 * PPQ);
    assert_eq!(
        flat(&d),
        before,
        "the notes stay where they were in song time"
    );
}

#[test]
fn a_looped_clip_wraps_its_loop_instead_of_shifting() {
    let mut c = notes_clip(BAR, 2 * BAR, vec![note(0, PPQ, 60, 0.8)]);
    c.loop_length = Some(BAR);
    let (mut d, r) = doc_with(vec![(0, c)]);
    one_step(&mut d, &r, trim(&[1], -PPQ));
    let c = clip_of(&d, 1);
    assert_eq!(c.offset, BAR - PPQ);
    assert_eq!(notes_of(&c), [(0, PPQ, 60)]);
}

#[test]
fn trimming_several_clips_is_one_undo_step_and_rejects_bad_input() {
    let (mut d, r) = doc_with(vec![
        (0, notes_clip(BAR, BAR, vec![note(0, PPQ, 60, 0.8)])),
        (0, notes_clip(3 * BAR, 2 * BAR, vec![note(0, PPQ, 61, 0.8)])),
        (
            1,
            auto_clip(BAR, 2 * BAR, vec![point(0, 100.0, Curve::Step)]),
        ),
    ]);
    one_step(&mut d, &r, trim(&[1, 2, 3], PPQ / 2));
    for id in 1..=3 {
        assert_eq!(clip_of(&d, id).offset, PPQ / 2);
    }
    // Below tick 0 and a length of 0 reject, and so does an unknown clip or a zero move.
    rejected(&mut d, &r, trim(&[1], -4 * BAR));
    rejected(&mut d, &r, trim(&[1], BAR));
    rejected(&mut d, &r, trim(&[9], PPQ));
    rejected(&mut d, &r, trim(&[1], 0));
}

#[test]
fn trimming_an_automation_clip_shifts_its_points_when_it_grows() {
    let (mut d, r) = doc_with(vec![
        (0, notes_clip(0, BAR, vec![])),
        (1, auto_clip(BAR, BAR, vec![point(0, 10.0, Curve::Step)])),
    ]);
    one_step(&mut d, &r, trim(&[2], -PPQ));
    let c = clip_of(&d, 2);
    let ClipContent::Automation(p) = &c.content else {
        panic!("automation")
    };
    assert_eq!((c.start, c.offset, p[0].tick), (BAR - PPQ, 0, PPQ));
}

proptest! {
    #[test]
    fn a_trimmed_clip_plays_what_the_old_one_played_inside_its_window(
        ticks in prop::collection::btree_set(0i64..BAR, 1..8),
        offset in 0i64..PPQ,
        delta in -(BAR / 2)..BAR,
    ) {
        let notes: Vec<_> = ticks
            .iter()
            .map(|t| note(*t, PPQ / 8, 60, 0.8))
            .collect();
        let mut c = notes_clip(BAR, BAR, notes);
        c.offset = offset;
        let (mut d, r) = doc_with(vec![(0, c)]);
        let before = flat(&d);
        // Growing further than the offset leaves new, empty ground; shrinking drops notes.
        d.apply(trim(&[1], delta), &r).unwrap();
        let new_start = BAR + delta;
        let expected: Vec<_> = before.iter().copied().filter(|n| n.0 >= new_start).collect();
        let got = flat(&d);
        if delta >= 0 {
            prop_assert_eq!(got, expected);
        } else {
            // Growing shows what lay before the offset: the old notes are all still there.
            prop_assert!(expected.iter().all(|n| got.contains(n)));
        }
        d.undo(&r).unwrap();
        prop_assert_eq!(flat(&d), before);
    }
}

// ---------------------------------------------------------------- delete outside events

#[test]
fn delete_outside_events_keeps_notes_in_the_window_and_a_boundary_point() {
    let mut n = notes_clip(
        BAR,
        BAR,
        vec![
            note(0, PPQ, 60, 0.8),
            note(PPQ, PPQ, 61, 0.8),
            note(BAR + PPQ, PPQ, 62, 0.8),
        ],
    );
    n.offset = PPQ;
    let mut a = auto_clip(
        BAR,
        BAR,
        vec![
            point(0, 1.0, Curve::Step),
            point(PPQ, 2.0, Curve::Step),
            point(PPQ + PPQ, 3.0, Curve::Step),
            point(5 * BAR, 4.0, Curve::Step),
            point(6 * BAR, 5.0, Curve::Step),
        ],
    );
    a.offset = 3 * PPQ;
    let (mut d, r) = doc_with(vec![(0, n), (1, a)]);
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DeleteOutsideEvents {
            ids: vec![ClipId(1), ClipId(2)],
        }),
    );
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert_eq!(notes_of(&clip_of(&d, 1)), [(PPQ, PPQ, 61)]);
    let c = clip_of(&d, 2);
    let ClipContent::Automation(p) = &c.content else {
        panic!("automation")
    };
    // Window [3 PPQ, 3 PPQ + BAR): the point before it and the one after stay, the rest go.
    let ticks: Vec<i64> = p.iter().map(|x| x.tick).collect();
    assert_eq!(ticks, [2 * PPQ, 5 * BAR]);
    // Nothing is left to delete now.
    rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DeleteOutsideEvents {
            ids: vec![ClipId(1)],
        }),
    );
}

// ---------------------------------------------------------------- split

#[test]
fn splitting_many_clips_is_one_undo_step_with_ids_in_song_order() {
    let (mut d, r) = doc_with(vec![
        (0, notes_clip(2 * BAR, 2 * BAR, vec![note(0, PPQ, 60, 0.8)])),
        (0, notes_clip(0, 5 * BAR, vec![note(BAR, PPQ, 61, 0.8)])),
        (1, auto_clip(BAR, 4 * BAR, vec![point(0, 1.0, Curve::Step)])),
    ]);
    let at = 3 * BAR;
    let before = flat(&d);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::SplitClips {
            ids: vec![ClipId(1), ClipId(2), ClipId(3)],
            at,
        }),
    );
    assert_eq!(flat(&d), before, "a split does not change what plays");
    // Song order: clip 2 (start 0), clip 3 (start BAR), clip 1 (start 2 BAR).
    assert_eq!(clip_of(&d, 4).start, at);
    assert_eq!(clip_of(&d, 4).offset, 3 * BAR);
    assert_eq!(clip_of(&d, 5).start, at);
    assert_eq!(clip_of(&d, 6).start, at);
    assert_eq!(clip_of(&d, 2).end(), at);
    assert_eq!(d.last_clip_id(), Some(ClipId(6)));
    // A point outside every clip rejects the command.
    rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::SplitClips {
            ids: vec![ClipId(1)],
            at: 100 * BAR,
        }),
    );
}

proptest! {
    #[test]
    fn splitting_at_any_tick_leaves_the_notes_unchanged(at in 1i64..(2 * BAR - 1)) {
        let (mut d, r) = doc_with(vec![(
            0,
            notes_clip(
                BAR,
                2 * BAR,
                vec![note(0, PPQ, 60, 0.8), note(BAR / 2, 2 * PPQ, 62, 0.8), note(BAR, PPQ, 64, 0.8)],
            ),
        )]);
        let before = flat(&d);
        d.apply(
            wf(WorkflowCommand::SplitClips { ids: vec![ClipId(1)], at: BAR + at }),
            &r,
        )
        .unwrap();
        // A note that crosses the cut is cut by the first half, then restarted from the second
        // half only where it starts: the starts and keys are what they were.
        let starts = |v: &[(i64, i64, u8, f32)]| v.iter().map(|n| (n.0, n.2)).collect::<Vec<_>>();
        let got = flat(&d);
        prop_assert_eq!(starts(&got), starts(&before));
        d.undo(&r).unwrap();
        prop_assert_eq!(flat(&d), before);
    }
}

// ---------------------------------------------------------------- cut out

#[test]
fn a_cut_out_leaves_the_clip_in_two_parts() {
    let (mut d, r) = doc_with(vec![(
        0,
        notes_clip(
            0,
            4 * BAR,
            vec![note(0, PPQ, 60, 0.8), note(2 * BAR, PPQ, 62, 0.8)],
        ),
    )]);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::CutOut {
            ids: vec![ClipId(1)],
            start: BAR,
            end: 3 * BAR,
            copy: false,
        }),
    );
    let (a, b) = (clip_of(&d, 1), clip_of(&d, 2));
    assert_eq!((a.start, a.end()), (0, BAR));
    assert_eq!((b.start, b.end(), b.offset), (3 * BAR, 4 * BAR, 3 * BAR));
    let got: Vec<i64> = flat(&d).iter().map(|n| n.0).collect();
    assert_eq!(got, [0], "the note inside the cut went with it");
}

#[test]
fn a_cut_out_over_a_whole_clip_removes_it_and_over_its_head_keeps_its_id() {
    let (mut d, r) = doc_with(vec![
        (0, notes_clip(BAR, BAR, vec![note(0, PPQ, 60, 0.8)])),
        (0, notes_clip(4 * BAR, 2 * BAR, vec![note(0, PPQ, 61, 0.8)])),
    ]);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::CutOut {
            ids: vec![ClipId(1), ClipId(2)],
            start: 0,
            end: 5 * BAR,
            copy: false,
        }),
    );
    assert!(d.doc().song.clip(ClipId(1)).is_none());
    let b = clip_of(&d, 2);
    assert_eq!((b.start, b.length, b.offset), (5 * BAR, BAR, BAR));
}

#[test]
fn a_cut_out_with_copy_keeps_the_originals_and_places_the_range_after_it() {
    let (mut d, r) = doc_with(vec![(
        0,
        notes_clip(0, 4 * BAR, vec![note(BAR, PPQ, 60, 0.8)]),
    )]);
    let original = clip_of(&d, 1);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::CutOut {
            ids: vec![ClipId(1)],
            start: BAR,
            end: 2 * BAR,
            copy: true,
        }),
    );
    assert_eq!(clip_of(&d, 1), original);
    let copy = clip_of(&d, 2);
    assert_eq!((copy.start, copy.length, copy.offset), (2 * BAR, BAR, BAR));
    rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::CutOut {
            ids: vec![ClipId(1)],
            start: 2 * BAR,
            end: 2 * BAR,
            copy: false,
        }),
    );
}

#[test]
fn set_clip_geometry_batches_still_work_next_to_the_new_commands() {
    // The nudge is a batch of geometry commands: one undo step.
    let (mut d, r) = doc_with(vec![
        (0, notes_clip(BAR, BAR, vec![])),
        (0, notes_clip(3 * BAR, BAR, vec![])),
    ]);
    let moves = [1u64, 2]
        .into_iter()
        .map(|id| {
            let c = clip_of(&d, id);
            sc(SongCommand::SetClipGeometry {
                id: ClipId(id),
                start: c.start + PPQ,
                length: c.length,
                offset: c.offset,
                loop_length: c.loop_length,
                muted: c.muted,
                merge: None,
            })
        })
        .collect();
    one_step(&mut d, &r, Command::Batch(moves));
}
