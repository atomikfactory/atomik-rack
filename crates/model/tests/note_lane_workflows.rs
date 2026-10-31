//! Note-lane workflows: moving and copying lanes, merging a track's lanes onto its top lane,
//! extracting notes and exploding a clip into lanes. Merge, explode and extract-move keep what
//! the track plays (the flattened notes and controller points), every command is one undo step
//! and the results survive a project round trip.

mod common;

use atomik_model::song::{flatten_controllers, flatten_notes_with};
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ControllerKind, ControllerLane, Curve, Document,
    ExtractMode, Lane, MAX_NOTE_LANES, SongCommand, Track, WorkflowCommand, empty_doc,
    load_project, save_project,
};
use common::song::*;
use common::*;
use proptest::prelude::*;
use std::collections::BTreeSet;

fn two_tracks(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("a")),
            add(1, SYNTH, Some("b")),
            add_track("a"),
            add_track("b"),
        ],
    );
    d
}

fn track<'a>(d: &'a Document, k: &str) -> &'a Track {
    d.doc().song.track(&key(k)).unwrap()
}

fn lane_cmd(k: &str) -> Command {
    sc(SongCommand::AddNoteLane {
        device: key(k),
        name: None,
    })
}

fn mv(k: &str, lane: usize, to: &str, to_index: usize, copy: bool) -> Command {
    sc(SongCommand::MoveNoteLane {
        device: key(k),
        lane,
        to_device: key(to),
        to_index,
        copy,
    })
}

fn merge(keys: &[&str]) -> Command {
    sc(SongCommand::MergeNoteLanes {
        devices: keys.iter().map(|k| key(k)).collect(),
    })
}

/// Applies `c`, checks it is one undo step and that the project survives a save and load.
fn step(d: &mut Document, r: &atomik_device_sdk::Registry, c: Command) {
    let before = d.doc().clone();
    d.apply(c, r).unwrap();
    let after = d.doc().clone();
    assert_ne!(before, after, "the command changed something");
    after.validate().unwrap();
    let text = save_project(&after);
    let loaded = load_project(&text, r).unwrap();
    assert_eq!(loaded.song, after.song, "the song survives a round trip");
    d.undo(r).expect("one step to undo");
    assert_eq!(d.doc(), &before);
    d.redo(r).expect("one step to redo");
    assert_eq!(d.doc(), &after);
}

fn refused(d: &mut Document, r: &atomik_device_sdk::Registry, c: Command) {
    let before = d.doc().clone();
    assert!(d.apply(c, r).is_err());
    assert_eq!(d.doc(), &before, "a refused command changes nothing");
}

fn lane_clip(d: &mut Document, r: &atomik_device_sdk::Registry, k: &str, lane: usize, c: Clip) {
    d.apply(add_clip(k, lane, c), r).unwrap();
}

fn pitches(lane: &Lane) -> Vec<u8> {
    let mut keys: Vec<u8> = lane
        .clips
        .iter()
        .flat_map(|c| match &c.content {
            ClipContent::Notes(n) => n.iter().map(|n| n.key).collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect();
    keys.sort_unstable();
    keys
}

// ---------------------------------------------------------------- merge

type Played = (
    Vec<(i64, i64, u8, f32)>,
    Vec<(ControllerKind, Vec<(i64, f32, Curve)>)>,
);

/// What a track plays, with notes of one tick and key (which a merged clip holds once) reduced
/// to the later lane's.
fn played(t: &Track, tails: bool) -> Played {
    let mut notes: Vec<(i64, i64, u8, f32)> = Vec::new();
    for n in flatten_notes_with(t, tails) {
        match notes.last_mut() {
            Some(last) if (last.0, last.2) == (n.0, n.2) => *last = n,
            _ => notes.push(n),
        }
    }
    (notes, flatten_controllers(t))
}

#[test]
fn merging_puts_the_audible_notes_of_every_lane_on_the_top_lane() {
    let r = registry();
    let mut d = two_tracks(&r);
    run(&mut d, &r, [lane_cmd("a"), lane_cmd("a")]);
    lane_clip(
        &mut d,
        &r,
        "a",
        0,
        notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.5)]),
    );
    lane_clip(
        &mut d,
        &r,
        "a",
        1,
        notes_clip(0, BAR, vec![note(PPQ, PPQ, 64, 0.6)]),
    );
    lane_clip(
        &mut d,
        &r,
        "a",
        2,
        notes_clip(2 * BAR, BAR, vec![note(0, PPQ, 67, 0.7)]),
    );
    let before = played(track(&d, "a"), false);
    step(&mut d, &r, merge(&["a"]));
    let t = track(&d, "a");
    assert_eq!(t.note_lane_count(), 1);
    assert_eq!(t.lanes[0].clips.len(), 2, "one clip per stretch of sound");
    assert_eq!(played(t, false), before);
}

#[test]
fn merging_drops_what_is_not_audible() {
    let r = registry();
    let mut d = two_tracks(&r);
    run(&mut d, &r, [lane_cmd("a"), lane_cmd("a")]);
    lane_clip(
        &mut d,
        &r,
        "a",
        0,
        notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.5)]),
    );
    // A muted lane, a muted clip and a clip hidden by a later one add nothing.
    lane_clip(
        &mut d,
        &r,
        "a",
        1,
        notes_clip(0, BAR, vec![note(0, PPQ, 62, 0.5)]),
    );
    lane_clip(
        &mut d,
        &r,
        "a",
        2,
        notes_clip(0, BAR, vec![note(0, PPQ, 64, 0.5)]),
    );
    run(
        &mut d,
        &r,
        [sc(SongCommand::SetLaneMuted {
            device: key("a"),
            lane: 2,
            muted: true,
        })],
    );
    let mut m = notes_clip(BAR, BAR, vec![note(0, PPQ, 65, 0.5)]);
    m.muted = true;
    lane_clip(&mut d, &r, "a", 1, m);
    let before = played(track(&d, "a"), false);
    step(&mut d, &r, merge(&["a"]));
    assert_eq!(played(track(&d, "a"), false), before);
    assert_eq!(pitches(&track(&d, "a").lanes[0]), vec![60, 62]);
}

#[test]
fn merging_refuses_a_track_with_one_lane_and_skips_it_among_several() {
    let r = registry();
    let mut d = two_tracks(&r);
    refused(&mut d, &r, merge(&["a"]));
    refused(&mut d, &r, merge(&[]));
    run(&mut d, &r, [lane_cmd("b")]);
    lane_clip(
        &mut d,
        &r,
        "b",
        1,
        notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.5)]),
    );
    step(&mut d, &r, merge(&["a", "b", "b"]));
    assert_eq!(track(&d, "b").note_lane_count(), 1);
}

#[test]
fn merging_keeps_the_name_of_the_top_lane_and_the_automation_lanes() {
    let r = registry();
    let mut d = two_tracks(&r);
    run(
        &mut d,
        &r,
        [
            sc(SongCommand::RenameLane {
                device: key("a"),
                lane: 0,
                name: Some("Lead".into()),
            }),
            lane_cmd("a"),
            sc(SongCommand::AddAutomationLane {
                device: key("a"),
                param: "filter.cutoff".into(),
            }),
        ],
    );
    step(&mut d, &r, merge(&["a"]));
    let t = track(&d, "a");
    assert_eq!(t.lanes.len(), 2);
    assert_eq!(t.lanes[0].name.as_deref(), Some("Lead"));
    assert!(!t.lanes[1].kind.is_notes());
}

#[test]
fn merging_two_notes_of_one_pitch_on_one_tick_keeps_the_later_lanes() {
    let r = registry();
    let mut d = two_tracks(&r);
    run(&mut d, &r, [lane_cmd("a")]);
    lane_clip(
        &mut d,
        &r,
        "a",
        0,
        notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.4)]),
    );
    lane_clip(
        &mut d,
        &r,
        "a",
        1,
        notes_clip(0, BAR, vec![note(0, PPQ / 2, 60, 0.9)]),
    );
    step(&mut d, &r, merge(&["a"]));
    let ClipContent::Notes(n) = &track(&d, "a").lanes[0].clips[0].content else {
        panic!("notes");
    };
    assert_eq!(n.len(), 1);
    assert_eq!((n[0].length, n[0].velocity), (PPQ / 2, 0.9));
}

type RawNote = (i64, u8);
type RawClip = (i64, i64, bool, BTreeSet<RawNote>, Option<i64>, i64, bool);

fn raw_clips() -> impl Strategy<Value = Vec<RawClip>> {
    prop::collection::vec(
        (
            (0i64..6).prop_map(|b| b * PPQ),
            (1i64..10).prop_map(|b| b * PPQ),
            prop::bool::weighted(0.15),
            prop::collection::btree_set(((0i64..8).prop_map(|b| b * PPQ / 2), 60u8..64), 0..5),
            proptest::option::of((1i64..4).prop_map(|b| b * PPQ)),
            (0i64..2).prop_map(|b| b * PPQ / 2),
            any::<bool>(),
        ),
        0..5,
    )
}

fn build_clip((start, len, muted, notes, lp, off, cc): &RawClip, lane_seed: usize) -> Clip {
    let ns = notes
        .iter()
        .map(|&(t, k)| {
            note(
                t,
                PPQ + (lane_seed as i64) * 60,
                k,
                0.5 + lane_seed as f32 * 0.1,
            )
        })
        .collect();
    let mut c = notes_clip(*start, *len, ns);
    c.muted = *muted;
    c.loop_length = *lp;
    c.offset = *off;
    if *cc {
        c.controllers = vec![ControllerLane::new(
            ControllerKind::Cc(1),
            vec![
                point(0, 0.1, Curve::Linear),
                point(PPQ, 0.9, Curve::Step),
                point(3 * PPQ, 0.4, Curve::Linear),
                point(5 * PPQ, 0.2, Curve::Step),
            ],
        )];
    }
    c
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// A merge leaves flatten_notes and flatten_controllers of the track as they were (but for
    /// the one-note-per-tick-and-key rule), with and without tails.
    #[test]
    fn merging_keeps_what_the_track_plays(
        lanes in prop::collection::vec((raw_clips(), prop::bool::weighted(0.2)), 2..5),
        tails in any::<bool>(),
    ) {
        let r = registry();
        let mut d = two_tracks(&r);
        let mut doc = d.doc().clone();
        doc.song_mut().tails = tails;
        d = Document::new(doc);
        for _ in 1..lanes.len() {
            d.apply(lane_cmd("a"), &r).unwrap();
        }
        for (i, (clips, muted)) in lanes.iter().enumerate() {
            for c in clips {
                d.apply(add_clip("a", i, build_clip(c, i)), &r).unwrap();
            }
            if *muted {
                d.apply(sc(SongCommand::SetLaneMuted { device: key("a"), lane: i, muted: true }), &r)
                    .unwrap();
            }
        }
        let before = played(track(&d, "a"), tails);
        d.apply(merge(&["a"]), &r).unwrap();
        d.doc().validate().unwrap();
        prop_assert_eq!(track(&d, "a").note_lane_count(), 1);
        prop_assert_eq!(played(track(&d, "a"), tails), before.clone());
        d.undo(&r).unwrap();
        prop_assert_eq!(played(track(&d, "a"), tails), before);
    }
}

// ---------------------------------------------------------------- move

#[test]
fn a_lane_reorders_within_its_track_and_keeps_its_clips() {
    let r = registry();
    let mut d = two_tracks(&r);
    run(&mut d, &r, [lane_cmd("a"), lane_cmd("a")]);
    for (lane, k) in [(0, 60), (1, 62), (2, 64)] {
        lane_clip(
            &mut d,
            &r,
            "a",
            lane,
            notes_clip(0, BAR, vec![note(0, PPQ, k, 0.5)]),
        );
    }
    let ids: Vec<_> = track(&d, "a").lanes.iter().map(|l| l.clips[0].id).collect();
    step(&mut d, &r, mv("a", 0, "a", 2, false));
    let t = track(&d, "a");
    let order: Vec<_> = t.lanes.iter().map(|l| l.clips[0].id).collect();
    assert_eq!(order, vec![ids[1], ids[2], ids[0]]);
    refused(&mut d, &r, mv("a", 2, "a", 2, false));
    refused(&mut d, &r, mv("a", 0, "a", 3, false));
}

#[test]
fn a_lane_moves_to_another_track_and_a_track_keeps_one_lane() {
    let r = registry();
    let mut d = two_tracks(&r);
    run(&mut d, &r, [lane_cmd("a")]);
    lane_clip(
        &mut d,
        &r,
        "a",
        1,
        notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.5)]),
    );
    let id = track(&d, "a").lanes[1].clips[0].id;
    refused(&mut d, &r, mv("b", 0, "a", 0, false));
    step(&mut d, &r, mv("a", 1, "b", 0, false));
    assert_eq!(track(&d, "a").note_lane_count(), 1);
    assert_eq!(track(&d, "b").note_lane_count(), 2);
    assert_eq!(track(&d, "b").lanes[0].clips[0].id, id);
}

#[test]
fn a_copied_lane_takes_new_clip_ids() {
    let r = registry();
    let mut d = two_tracks(&r);
    lane_clip(
        &mut d,
        &r,
        "a",
        0,
        notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.5)]),
    );
    lane_clip(
        &mut d,
        &r,
        "a",
        0,
        notes_clip(BAR, BAR, vec![note(0, PPQ, 62, 0.5)]),
    );
    let old: Vec<_> = track(&d, "a").lanes[0].clips.iter().map(|c| c.id).collect();
    step(&mut d, &r, mv("a", 0, "b", 1, true));
    refused(&mut d, &r, mv("a", 0, "b", 3, true));
    let a = track(&d, "a");
    assert_eq!(
        a.lanes[0].clips.iter().map(|c| c.id).collect::<Vec<_>>(),
        old
    );
    let b = track(&d, "b");
    assert_eq!(b.note_lane_count(), 2);
    let new: Vec<_> = b.lanes[1].clips.iter().map(|c| c.id).collect();
    assert!(new.iter().all(|id| !old.contains(id)));
    let all = clip_ids(d.doc());
    let unique: BTreeSet<_> = all.iter().collect();
    assert_eq!(all.len(), unique.len(), "clip ids stay unique");
}

#[test]
fn lane_moves_respect_the_lane_limit() {
    let r = registry();
    let mut d = two_tracks(&r);
    for _ in 1..MAX_NOTE_LANES {
        d.apply(lane_cmd("b"), &r).unwrap();
    }
    refused(&mut d, &r, mv("a", 0, "b", 0, true));
    d.apply(lane_cmd("a"), &r).unwrap();
    refused(&mut d, &r, mv("a", 1, "b", 0, false));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Moving a lane to another track or place keeps the notes the tracks play together.
    #[test]
    fn moving_a_lane_keeps_every_note(
        lanes in prop::collection::vec(raw_clips(), 2..5),
        from in 0usize..5,
        to in 0usize..5,
        other in any::<bool>(),
    ) {
        let r = registry();
        let mut d = two_tracks(&r);
        for _ in 1..lanes.len() {
            d.apply(lane_cmd("a"), &r).unwrap();
        }
        for (i, clips) in lanes.iter().enumerate() {
            for c in clips {
                d.apply(add_clip("a", i, build_clip(c, i)), &r).unwrap();
            }
        }
        let all = |d: &Document| {
            let mut n: Vec<_> = ["a", "b"]
                .iter()
                .flat_map(|k| flatten_notes_with(track(d, k), false))
                .collect();
            n.sort_by(|x, y| x.partial_cmp(y).unwrap());
            n
        };
        let before = all(&d);
        let from = from % lanes.len();
        let target = if other { "b" } else { "a" };
        let to = to % (if other { 2 } else { lanes.len() });
        if d.apply(mv("a", from, target, to, false), &r).is_ok() {
            d.doc().validate().unwrap();
            prop_assert_eq!(all(&d), before);
        }
    }
}

// ---------------------------------------------------------------- extract and explode

fn five_pitch_clip() -> Clip {
    notes_clip(
        BAR,
        2 * BAR,
        vec![
            note(0, PPQ, 60, 0.5),
            note(0, PPQ, 64, 0.6),
            note(PPQ, PPQ, 60, 0.7),
            note(PPQ, PPQ, 67, 0.8),
            note(2 * PPQ, 3 * PPQ, 62, 0.9),
            note(3 * PPQ, PPQ, 71, 0.4),
        ],
    )
}

#[test]
fn exploding_a_clip_gives_each_pitch_a_lane_and_plays_the_same() {
    let r = registry();
    let mut d = two_tracks(&r);
    let mut c = five_pitch_clip();
    c.controllers = vec![ControllerLane::new(
        ControllerKind::Cc(1),
        vec![point(0, 0.2, Curve::Step)],
    )];
    lane_clip(&mut d, &r, "a", 0, c);
    let id = track(&d, "a").lanes[0].clips[0].id;
    let before = played(track(&d, "a"), false);
    step(
        &mut d,
        &r,
        sc_workflow(WorkflowCommand::ExplodeNotes { clip: id }),
    );
    let t = track(&d, "a");
    assert_eq!(t.note_lane_count(), 6, "the source lane and one per pitch");
    assert_eq!(played(t, false), before);
    assert_eq!(t.lanes[1].name.as_deref(), Some("C3"));
    assert_eq!(pitches(&t.lanes[0]), Vec::<u8>::new());
    assert_eq!(t.lanes[0].clips[0].controllers.len(), 1, "controllers stay");
    assert_eq!(pitches(&t.lanes[5]), vec![71]);
    let lanes: Vec<_> = (1..6).map(|i| t.lanes[i].clips[0].start).collect();
    assert_eq!(lanes, vec![BAR; 5]);
    assert!(d.last_clip_id().is_some());
}

fn sc_workflow(w: WorkflowCommand) -> Command {
    Command::Workflow(w)
}

#[test]
fn exploding_refuses_a_single_pitch_a_hidden_clip_and_too_many_lanes() {
    let r = registry();
    let mut d = two_tracks(&r);
    lane_clip(
        &mut d,
        &r,
        "a",
        0,
        notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.5)]),
    );
    let id = track(&d, "a").lanes[0].clips[0].id;
    refused(
        &mut d,
        &r,
        sc_workflow(WorkflowCommand::ExplodeNotes { clip: id }),
    );
    lane_clip(
        &mut d,
        &r,
        "b",
        0,
        notes_clip(
            0,
            2 * BAR,
            vec![note(0, PPQ, 60, 0.5), note(PPQ, PPQ, 62, 0.5)],
        ),
    );
    lane_clip(&mut d, &r, "b", 0, notes_clip(BAR, BAR, Vec::new()));
    let hidden = track(&d, "b").lanes[0].clips[0].id;
    refused(
        &mut d,
        &r,
        sc_workflow(WorkflowCommand::ExplodeNotes { clip: hidden }),
    );
}

#[test]
fn extracting_moves_the_selected_notes_to_a_new_lane() {
    let r = registry();
    let mut d = two_tracks(&r);
    lane_clip(&mut d, &r, "a", 0, five_pitch_clip());
    let id = track(&d, "a").lanes[0].clips[0].id;
    let before = played(track(&d, "a"), false);
    let extract = |selection: Vec<usize>, mode, to| {
        sc_workflow(WorkflowCommand::ExtractNotes {
            clip: id,
            selection,
            mode,
            to,
        })
    };
    step(&mut d, &r, extract(vec![0, 2], ExtractMode::Move, None));
    let t = track(&d, "a");
    assert_eq!(t.note_lane_count(), 2);
    assert_eq!(pitches(&t.lanes[0]), vec![62, 64, 67, 71]);
    assert_eq!(pitches(&t.lanes[1]), vec![60, 60]);
    assert_eq!(t.lanes[1].name.as_deref(), Some("C3"));
    assert_eq!(played(t, false), before, "a move keeps the sound");
    // A copy leaves the notes where they are.
    d.undo(&r).unwrap();
    d.apply(extract(vec![1], ExtractMode::Copy, None), &r)
        .unwrap();
    let t = track(&d, "a");
    assert_eq!(pitches(&t.lanes[0]), vec![60, 60, 62, 64, 67, 71]);
    assert_eq!(pitches(&t.lanes[1]), vec![64]);
    d.undo(&r).unwrap();
    refused(&mut d, &r, extract(vec![], ExtractMode::Move, None));
    refused(&mut d, &r, extract(vec![9], ExtractMode::Move, None));
    refused(&mut d, &r, extract(vec![0], ExtractMode::Move, Some(0)));
}

#[test]
fn extracting_to_an_existing_lane_needs_an_empty_place() {
    let r = registry();
    let mut d = two_tracks(&r);
    run(&mut d, &r, [lane_cmd("a")]);
    lane_clip(&mut d, &r, "a", 0, five_pitch_clip());
    let id = track(&d, "a").lanes[0].clips[0].id;
    let to = |lane| {
        sc_workflow(WorkflowCommand::ExtractNotes {
            clip: id,
            selection: vec![0],
            mode: ExtractMode::Move,
            to: Some(lane),
        })
    };
    step(&mut d, &r, to(1));
    refused(&mut d, &r, to(1));
    refused(&mut d, &r, to(5));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Exploding a clip and extracting notes with a move keep what the track plays.
    #[test]
    fn exploding_and_extracting_keep_what_the_track_plays(
        raw in raw_clips().prop_filter("one clip", |c| !c.is_empty()),
        pick in prop::collection::btree_set(0usize..6, 1..4),
        tails in any::<bool>(),
    ) {
        let r = registry();
        let mut doc = empty_doc();
        doc.song_mut().tails = tails;
        let mut d = Document::new(doc);
        run(&mut d, &r, [add(0, SYNTH, Some("a")), add_track("a")]);
        let mut c = build_clip(&raw[0], 0);
        c.muted = false;
        d.apply(add_clip("a", 0, c), &r).unwrap();
        let id = ClipId(1);
        let before = played(track(&d, "a"), tails);
        let n = match &track(&d, "a").lanes[0].clips[0].content {
            ClipContent::Notes(n) => n.len(),
            _ => unreachable!(),
        };
        let extract = sc_workflow(WorkflowCommand::ExtractNotes {
            clip: id,
            selection: pick.iter().copied().filter(|&i| i < n).collect(),
            mode: ExtractMode::Move,
            to: None,
        });
        if d.apply(extract, &r).is_ok() {
            d.doc().validate().unwrap();
            prop_assert_eq!(played(track(&d, "a"), tails), before.clone());
            d.undo(&r).unwrap();
        }
        if d.apply(sc_workflow(WorkflowCommand::ExplodeNotes { clip: id }), &r).is_ok() {
            d.doc().validate().unwrap();
            prop_assert_eq!(played(track(&d, "a"), tails), before);
        }
    }
}
