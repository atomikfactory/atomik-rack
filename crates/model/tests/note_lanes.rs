//! Note lanes: a track holds one or more note lanes, then its automation lanes. Constructors,
//! names and mute, the four commands, and where moved and pasted clips land.

mod common;

use atomik_model::{
    ClipId, ClipMove, ClipSnapshot, Command, Curve, Document, Lane, LaneKind, MAX_LANE_NAME_CHARS,
    MAX_NOTE_LANES, ModelError, SongCommand, SyncOp, Track, WorkflowCommand, empty_doc,
};
use common::audio::*;
use common::song::*;
use common::workflow::*;
use common::*;
use proptest::prelude::*;

const CUTOFF: &str = "filter.cutoff";

#[test]
fn lane_constructors_build_empty_lanes_of_each_kind() {
    for (lane, kind) in [
        (Lane::notes(), LaneKind::Notes),
        (Lane::audio(), LaneKind::Audio),
        (
            Lane::automation("filter.cutoff"),
            LaneKind::Automation("filter.cutoff".into()),
        ),
        (Lane::new(LaneKind::Notes), LaneKind::Notes),
    ] {
        assert_eq!(lane.kind, kind);
        assert!(lane.clips.is_empty() && lane.take_groups.is_empty());
        assert_eq!((lane.name.as_deref(), lane.muted), (None, false));
    }
}

/// A synth track `a` with its note lane and a cutoff automation lane; a second track `b`.
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
            sc(SongCommand::AddAutomationLane {
                device: key("a"),
                param: CUTOFF.into(),
            }),
        ],
    );
    d
}

fn add_lane(k: &str, name: Option<&str>) -> Command {
    sc(SongCommand::AddNoteLane {
        device: key(k),
        name: name.map(Into::into),
    })
}

fn remove_lane(k: &str, lane: usize) -> Command {
    sc(SongCommand::RemoveNoteLane {
        device: key(k),
        lane,
    })
}

fn rename(k: &str, lane: usize, name: Option<&str>) -> Command {
    sc(SongCommand::RenameLane {
        device: key(k),
        lane,
        name: name.map(Into::into),
    })
}

fn mute(k: &str, lane: usize, muted: bool) -> Command {
    sc(SongCommand::SetLaneMuted {
        device: key(k),
        lane,
        muted,
    })
}

/// Applies `c` and checks that it is one undo step: undo restores the document before, redo
/// the document after. Returns the sync ops of the first application.
fn step(d: &mut Document, r: &atomik_device_sdk::Registry, c: Command) -> Vec<SyncOp> {
    let before = d.doc().clone();
    let ops = d.apply(c, r).unwrap();
    let after = d.doc().clone();
    assert_ne!(before, after, "the command changed something");
    d.undo(r).expect("one step to undo");
    assert_eq!(d.doc(), &before);
    d.redo(r).expect("one step to redo");
    assert_eq!(d.doc(), &after);
    ops
}

fn track<'a>(d: &'a Document, k: &str) -> &'a Track {
    d.doc().song.track(&key(k)).unwrap()
}

/// The lane kinds of a track as short tags: `n` notes, `a` automation, `u` audio.
fn shape(d: &Document, k: &str) -> String {
    track(d, k)
        .lanes
        .iter()
        .map(|l| match l.kind {
            LaneKind::Notes => 'n',
            LaneKind::Automation(_) => 'a',
            LaneKind::Audio => 'u',
        })
        .collect()
}

fn clip_on(
    d: &mut Document,
    r: &atomik_device_sdk::Registry,
    k: &str,
    lane: usize,
    at: i64,
) -> ClipId {
    d.apply(
        add_clip(k, lane, notes_clip(at, BAR, vec![note(0, PPQ, 60, 0.8)])),
        r,
    )
    .unwrap();
    last(d)
}

fn lane_of(d: &Document, id: ClipId) -> (String, usize) {
    let (t, lane, _) = d.doc().song.clip(id).unwrap();
    (t.device.0.clone(), lane)
}

// ---------------------------------------------------------------- helpers

#[test]
fn track_helpers_count_note_lanes_and_label_them() {
    let r = registry();
    let mut d = arranged(&r);
    d.apply(add_lane("a", None), &r).unwrap();
    d.apply(add_lane("a", Some("Bass")), &r).unwrap();
    let t = track(&d, "a");
    assert_eq!(t.note_lane_count(), 3);
    assert_eq!(t.note_lanes().count(), 3);
    assert!(t.is_note_lane(0) && t.is_note_lane(2));
    assert!(!t.is_note_lane(3) && !t.is_note_lane(4));
    assert_eq!(t.lane_label(0), "Notes");
    assert_eq!(t.lane_label(1), "Notes 2");
    assert_eq!(t.lane_label(2), "Bass");
    assert_eq!(t.lane_label(3), CUTOFF);
}

// ---------------------------------------------------------------- add

#[test]
fn add_note_lane_inserts_after_the_last_note_lane_before_automation_in_one_undo_step() {
    let r = registry();
    let mut d = arranged(&r);
    assert_eq!(shape(&d, "a"), "na");
    let ops = step(&mut d, &r, add_lane("a", None));
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert_eq!(shape(&d, "a"), "nna");
    d.apply(add_lane("a", None), &r).unwrap();
    assert_eq!(shape(&d, "a"), "nnna");
}

#[test]
fn add_note_lane_names_the_lane_trimmed_and_blank_means_unnamed() {
    let r = registry();
    let mut d = arranged(&r);
    d.apply(add_lane("b", Some("  Pad  ")), &r).unwrap();
    d.apply(add_lane("b", Some("   ")), &r).unwrap();
    d.apply(add_lane("b", None), &r).unwrap();
    let names: Vec<Option<&str>> = track(&d, "b")
        .lanes
        .iter()
        .map(|l| l.name.as_deref())
        .collect();
    assert_eq!(names, [None, Some("Pad"), None, None]);
}

#[test]
fn add_note_lane_rejects_bad_names_audio_tracks_unknown_tracks_and_the_limit() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    run(&mut d, &r, [add(1, SYNTH, Some("a")), add_track("a")]);
    let long = "x".repeat(MAX_LANE_NAME_CHARS + 1);
    let e = rejected(&mut d, &r, add_lane("a", Some(&long)));
    assert!(e.to_string().contains("at most 32 characters"), "{e}");
    let e = rejected(&mut d, &r, add_lane("a", Some("bad\nname")));
    assert!(e.to_string().contains("control characters"), "{e}");
    rejected(&mut d, &r, add_lane("nope", None));
    let audio = d.doc().song.tracks[0].device.0.clone();
    let e = rejected(&mut d, &r, add_lane(&audio, None));
    assert!(e.to_string().contains("audio track"), "{e}");
    for _ in 1..MAX_NOTE_LANES {
        d.apply(add_lane("a", None), &r).unwrap();
    }
    assert_eq!(track(&d, "a").note_lane_count(), MAX_NOTE_LANES);
    let e = rejected(&mut d, &r, add_lane("a", None));
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e}");
}

// ---------------------------------------------------------------- remove

#[test]
fn remove_note_lane_takes_its_clips_and_one_undo_step_brings_them_back() {
    let r = registry();
    let mut d = arranged(&r);
    d.apply(add_lane("a", Some("Two")), &r).unwrap();
    let on_two = clip_on(&mut d, &r, "a", 1, 0);
    let on_one = clip_on(&mut d, &r, "a", 0, 0);
    let ops = step(&mut d, &r, remove_lane("a", 1));
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert_eq!(shape(&d, "a"), "na");
    assert!(d.doc().song.clip(on_two).is_none());
    assert_eq!(lane_of(&d, on_one), ("a".into(), 0));
    d.undo(&r).unwrap();
    assert_eq!(lane_of(&d, on_two), ("a".into(), 1));
    assert_eq!(track(&d, "a").lanes[1].name.as_deref(), Some("Two"));
}

#[test]
fn remove_note_lane_rejects_the_last_note_lane_automation_and_missing_lanes() {
    let r = registry();
    let mut d = arranged(&r);
    let e = rejected(&mut d, &r, remove_lane("a", 0));
    assert!(e.to_string().contains("at least one note lane"), "{e}");
    let e = rejected(&mut d, &r, remove_lane("a", 1));
    assert!(e.to_string().contains("not a note lane"), "{e}");
    let e = rejected(&mut d, &r, remove_lane("a", 9));
    assert!(e.to_string().contains("no lane 9"), "{e}");
    rejected(&mut d, &r, remove_lane("nope", 0));
}

// ---------------------------------------------------------------- rename and mute

#[test]
fn rename_lane_sets_trims_and_resets_the_name_as_one_undo_step() {
    let r = registry();
    let mut d = arranged(&r);
    let ops = step(&mut d, &r, rename("a", 0, Some(" Lead ")));
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert_eq!(track(&d, "a").lanes[0].name.as_deref(), Some("Lead"));
    assert_eq!(track(&d, "a").lane_label(0), "Lead");
    step(&mut d, &r, rename("a", 0, Some("   ")));
    assert_eq!(track(&d, "a").lanes[0].name, None);
    d.apply(rename("a", 0, Some("Lead")), &r).unwrap();
    d.apply(rename("a", 0, None), &r).unwrap();
    assert_eq!(track(&d, "a").lane_label(0), "Notes");
}

#[test]
fn rename_lane_rejects_bad_names_and_lanes_that_are_not_note_lanes() {
    let r = registry();
    let mut d = arranged(&r);
    let long = "x".repeat(MAX_LANE_NAME_CHARS + 1);
    rejected(&mut d, &r, rename("a", 0, Some(&long)));
    rejected(&mut d, &r, rename("a", 0, Some("a\tb")));
    let e = rejected(&mut d, &r, rename("a", 1, Some("Cut")));
    assert!(e.to_string().contains("not a note lane"), "{e}");
    rejected(&mut d, &r, rename("a", 7, Some("Cut")));
    rejected(&mut d, &r, rename("nope", 0, Some("Cut")));
    let ok = "y".repeat(MAX_LANE_NAME_CHARS);
    d.apply(rename("a", 0, Some(&ok)), &r).unwrap();
}

#[test]
fn set_lane_muted_toggles_one_lane_reports_song_changed_and_undoes() {
    let r = registry();
    let mut d = arranged(&r);
    d.apply(add_lane("a", None), &r).unwrap();
    let ops = step(&mut d, &r, mute("a", 1, true));
    assert_eq!(ops, [SyncOp::SongChanged]);
    let t = track(&d, "a");
    assert!(t.lanes[1].muted && !t.lanes[0].muted);
    step(&mut d, &r, mute("a", 1, false));
    assert!(!track(&d, "a").lanes[1].muted);
}

#[test]
fn set_lane_muted_rejects_a_no_op_automation_lanes_and_missing_lanes() {
    let r = registry();
    let mut d = arranged(&r);
    let e = rejected(&mut d, &r, mute("a", 0, false));
    assert!(e.to_string().contains("already"), "{e}");
    d.apply(mute("a", 0, true), &r).unwrap();
    let e = rejected(&mut d, &r, mute("a", 0, true));
    assert!(e.to_string().contains("already"), "{e}");
    let e = rejected(&mut d, &r, mute("a", 1, true));
    assert!(e.to_string().contains("not a note lane"), "{e}");
    rejected(&mut d, &r, mute("a", 5, true));
}

#[test]
fn add_note_lane_leaves_a_muted_lane_zero_muted() {
    let r = registry();
    let mut d = arranged(&r);
    d.apply(mute("a", 0, true), &r).unwrap();
    d.apply(add_lane("a", None), &r).unwrap();
    assert!(track(&d, "a").lanes[0].muted && !track(&d, "a").lanes[1].muted);
}

// ---------------------------------------------------------------- move

fn mv(id: ClipId, track: &str, lane: Option<usize>, start: i64) -> ClipMove {
    ClipMove {
        id,
        track: key(track),
        lane,
        start,
    }
}

fn move_cmd(moves: Vec<ClipMove>, copy: bool) -> Command {
    wf(WorkflowCommand::MoveClips {
        moves,
        copy,
        merge: None,
    })
}

/// Tracks a (three note lanes, then cutoff) and b (two note lanes).
fn laned(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = arranged(r);
    run(
        &mut d,
        r,
        [
            add_lane("a", None),
            add_lane("a", None),
            add_lane("b", None),
        ],
    );
    d
}

#[test]
fn move_clips_without_a_lane_on_the_same_track_keeps_the_source_lane() {
    let r = registry();
    let mut d = laned(&r);
    let id = clip_on(&mut d, &r, "a", 2, 0);
    step(
        &mut d,
        &r,
        move_cmd(vec![mv(id, "a", None, 2 * BAR)], false),
    );
    assert_eq!(lane_of(&d, id), ("a".into(), 2));
    assert_eq!(doc_clip(d.doc(), id).start, 2 * BAR);
}

#[test]
fn move_clips_with_a_lane_puts_the_clip_on_that_note_lane() {
    let r = registry();
    let mut d = laned(&r);
    let id = clip_on(&mut d, &r, "a", 0, 0);
    step(&mut d, &r, move_cmd(vec![mv(id, "a", Some(2), 0)], false));
    assert_eq!(lane_of(&d, id), ("a".into(), 2));
    step(&mut d, &r, move_cmd(vec![mv(id, "b", Some(1), BAR)], false));
    assert_eq!(lane_of(&d, id), ("b".into(), 1));
}

#[test]
fn move_clips_to_another_track_without_a_lane_keeps_the_note_lane_ordinal_clamped() {
    let r = registry();
    let mut d = laned(&r);
    let low = clip_on(&mut d, &r, "a", 1, 0);
    let high = clip_on(&mut d, &r, "a", 2, BAR);
    step(
        &mut d,
        &r,
        move_cmd(vec![mv(low, "b", None, 0), mv(high, "b", None, BAR)], false),
    );
    assert_eq!(lane_of(&d, low), ("b".into(), 1));
    // Track b has two note lanes, so lane 2 clamps to its last.
    assert_eq!(lane_of(&d, high), ("b".into(), 1));
}

#[test]
fn move_clips_copy_with_a_lane_keeps_the_source_and_adds_the_copy_there() {
    let r = registry();
    let mut d = laned(&r);
    let id = clip_on(&mut d, &r, "a", 0, 0);
    step(&mut d, &r, move_cmd(vec![mv(id, "a", Some(1), 0)], true));
    assert_eq!(lane_of(&d, id), ("a".into(), 0));
    assert_eq!(track(&d, "a").lanes[1].clips.len(), 1);
}

#[test]
fn move_clips_rejects_a_lane_of_another_kind_or_one_that_does_not_exist() {
    let r = registry();
    let mut d = laned(&r);
    let id = clip_on(&mut d, &r, "a", 0, 0);
    // Lane 3 of a is the cutoff automation lane.
    let e = rejected(&mut d, &r, move_cmd(vec![mv(id, "a", Some(3), 0)], false));
    assert!(e.to_string().contains("not a note lane"), "{e}");
    let e = rejected(&mut d, &r, move_cmd(vec![mv(id, "b", Some(5), 0)], false));
    assert!(e.to_string().contains("no lane 5"), "{e}");
}

#[test]
fn move_clips_an_automation_clip_keeps_the_lane_of_its_param() {
    let r = registry();
    let mut d = laned(&r);
    d.apply(
        add_clip("a", 3, auto_clip(0, BAR, vec![point(0, 0.5, Curve::Step)])),
        &r,
    )
    .unwrap();
    let id = last(&d);
    step(&mut d, &r, move_cmd(vec![mv(id, "a", None, BAR)], false));
    assert_eq!(lane_of(&d, id), ("a".into(), 3));
}

// ---------------------------------------------------------------- paste

fn paste(track: Option<&str>, lane: Option<usize>, s: &ClipSnapshot) -> Command {
    wf(WorkflowCommand::PasteClips {
        track: track.map(key),
        lane,
        at_tick: 8 * BAR,
        clips: s.clone(),
    })
}

#[test]
fn a_clip_snapshot_records_the_note_lane_ordinal_of_each_clip() {
    let r = registry();
    let mut d = laned(&r);
    let one = clip_on(&mut d, &r, "a", 1, 0);
    let zero = clip_on(&mut d, &r, "a", 0, BAR);
    let s = ClipSnapshot::capture(d.doc(), &[one, zero]).unwrap();
    let ord: Vec<usize> = s.entries.iter().map(|e| e.note_lane).collect();
    assert_eq!(ord, [1, 0]);
}

#[test]
fn pasting_without_a_lane_puts_each_clip_back_on_its_note_lane_ordinal() {
    let r = registry();
    let mut d = laned(&r);
    let one = clip_on(&mut d, &r, "a", 1, 0);
    let two = clip_on(&mut d, &r, "a", 2, BAR);
    let s = ClipSnapshot::capture(d.doc(), &[one, two]).unwrap();
    step(&mut d, &r, paste(None, None, &s));
    assert_eq!(track(&d, "a").lanes[1].clips.len(), 2);
    assert_eq!(track(&d, "a").lanes[2].clips.len(), 2);
    assert!(track(&d, "a").lanes[0].clips.is_empty());
    // On track b (two note lanes) ordinal 2 clamps to the last note lane.
    step(&mut d, &r, paste(Some("b"), None, &s));
    assert_eq!(track(&d, "b").lanes[1].clips.len(), 2);
}

#[test]
fn pasting_with_a_lane_puts_every_note_clip_on_that_lane() {
    let r = registry();
    let mut d = laned(&r);
    let one = clip_on(&mut d, &r, "a", 1, 0);
    let two = clip_on(&mut d, &r, "a", 2, BAR);
    let s = ClipSnapshot::capture(d.doc(), &[one, two]).unwrap();
    step(&mut d, &r, paste(None, Some(0), &s));
    assert_eq!(track(&d, "a").lanes[0].clips.len(), 2);
    let e = rejected(&mut d, &r, paste(None, Some(3), &s));
    assert!(e.to_string().contains("no note lane 3"), "{e}");
}

#[test]
fn duplicating_clips_keeps_them_on_their_note_lane() {
    let r = registry();
    let mut d = laned(&r);
    let two = clip_on(&mut d, &r, "a", 2, 0);
    step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateClips { ids: vec![two] }),
    );
    assert_eq!(track(&d, "a").lanes[2].clips.len(), 2);
}

// ---------------------------------------------------------------- properties

#[derive(Debug, Clone)]
enum Op {
    Add(bool),
    Remove(usize),
    Rename(usize, bool),
    Mute(usize, bool),
    Clip(usize, i64),
    Move(usize, usize, bool),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => any::<bool>().prop_map(Op::Add),
        2 => (0usize..6).prop_map(Op::Remove),
        1 => (0usize..6, any::<bool>()).prop_map(|(l, n)| Op::Rename(l, n)),
        1 => (0usize..6, any::<bool>()).prop_map(|(l, m)| Op::Mute(l, m)),
        3 => (0usize..6, 0i64..8).prop_map(|(l, b)| Op::Clip(l, b * BAR)),
        3 => (0usize..8, 0usize..6, any::<bool>()).prop_map(|(c, l, other)| Op::Move(c, l, other)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Random lane adds, removals, renames, mutes, clips and moves keep the note lanes in one
    /// leading block of at least one lane, and undoing every applied command restores the
    /// document.
    #[test]
    fn random_lane_commands_keep_the_note_lanes_contiguous_and_undo_all_restores(
        ops in prop::collection::vec(op(), 1..40)
    ) {
        let r = registry();
        let mut d = arranged(&r);
        let start = text(&d);
        let mut applied = 0;
        for o in ops {
            let t = track(&d, "a");
            let n = t.lanes.len();
            let clips: Vec<ClipId> = t
                .lanes
                .iter()
                .flat_map(|l| l.clips.iter().map(|c| c.id))
                .collect();
            let cmd = match o {
                Op::Add(named) => add_lane("a", named.then_some("N")),
                Op::Remove(l) => remove_lane("a", l % n),
                Op::Rename(l, set) => rename("a", l % n, set.then_some("R")),
                Op::Mute(l, m) => mute("a", l % n, m),
                Op::Clip(l, at) => add_clip(
                    "a",
                    l % t.note_lane_count(),
                    notes_clip(at, BAR, vec![note(0, PPQ, 60, 0.8)]),
                ),
                Op::Move(c, l, other) => {
                    if clips.is_empty() {
                        continue;
                    }
                    let id = clips[c % clips.len()];
                    let (_, from, _) = d.doc().song.clip(id).unwrap();
                    let lane = t.lanes[from].kind.is_notes().then_some(l % t.note_lane_count());
                    move_cmd(vec![mv(id, if other { "b" } else { "a" }, lane, BAR)], false)
                }
            };
            if d.apply(cmd, &r).is_ok() {
                applied += 1;
            }
            for k in ["a", "b"] {
                let t = track(&d, k);
                let count = t.note_lane_count();
                prop_assert!((1..=MAX_NOTE_LANES).contains(&count));
                prop_assert!(t.lanes[..count].iter().all(|l| l.kind.is_notes()));
                prop_assert!(t.lanes[count..].iter().all(|l| !l.kind.is_notes()));
            }
        }
        for _ in 0..applied {
            prop_assert!(d.undo(&r).is_some());
        }
        prop_assert_eq!(text(&d), start);
    }
}
