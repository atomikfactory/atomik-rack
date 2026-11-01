//! Flattening of a track with several note lanes: the notes and the controller data of the
//! unmuted lanes merge, a muted lane plays nothing, and splitting clips across lanes does not
//! change what plays.

mod common;

use atomik_model::song::{flatten_automation, flatten_controllers, flatten_notes};
use atomik_model::{Clip, ClipId, ControllerKind, ControllerLane, Curve, Lane, Note, Track};
use common::key;
use common::song::*;
use proptest::prelude::*;

fn with_id(mut c: Clip, id: u64) -> Clip {
    c.id = ClipId(id);
    c
}

fn track_of(lanes: Vec<Lane>) -> Track {
    Track {
        device: key("synth"),
        muted: false,
        solo: false,
        armed: false,
        chase: true,
        notes_to: None,
        direct_record: false,
        frozen: None,
        lanes,
    }
}

fn muted(mut lane: Lane) -> Lane {
    lane.muted = true;
    lane
}

fn lane_with(clips: Vec<Clip>) -> Lane {
    Lane::notes().with_clips(clips)
}

fn with_controllers(mut c: Clip, lanes: Vec<ControllerLane>) -> Clip {
    c.controllers = lanes;
    c
}

fn cc(n: u8, points: Vec<(i64, f32)>) -> ControllerLane {
    ControllerLane::new(
        ControllerKind::Cc(n),
        points
            .into_iter()
            .map(|(t, v)| point(t, v, Curve::Step))
            .collect(),
    )
}

// ---- notes ----------------------------------------------------------------------------------

#[test]
fn flatten_notes_merges_the_notes_of_every_lane_sorted_by_tick_then_key() {
    let a = with_id(notes_clip(0, BAR, vec![note(100, 10, 64, 0.5)]), 1);
    let b = with_id(
        notes_clip(
            0,
            BAR,
            vec![note(50, 10, 72, 0.25), note(100, 10, 60, 0.75)],
        ),
        2,
    );
    let t = track_of(vec![lane_with(vec![a]), lane_with(vec![b])]);
    assert_eq!(
        flatten_notes(&t),
        [(50, 10, 72, 0.25), (100, 10, 60, 0.75), (100, 10, 64, 0.5)]
    );
}

#[test]
fn flatten_notes_a_muted_lane_plays_nothing() {
    let a = with_id(notes_clip(0, BAR, vec![note(0, 10, 60, 0.5)]), 1);
    let b = with_id(notes_clip(0, BAR, vec![note(0, 10, 70, 0.5)]), 2);
    let t = track_of(vec![lane_with(vec![a]), muted(lane_with(vec![b]))]);
    assert_eq!(flatten_notes(&t), [(0, 10, 60, 0.5)]);
    let first_muted = track_of(vec![
        muted(lane_with(vec![with_id(
            notes_clip(0, BAR, vec![note(0, 10, 60, 0.5)]),
            1,
        )])),
        lane_with(vec![with_id(
            notes_clip(0, BAR, vec![note(0, 10, 70, 0.5)]),
            2,
        )]),
    ]);
    assert_eq!(flatten_notes(&first_muted), [(0, 10, 70, 0.5)]);
    let all_muted = track_of(vec![muted(lane_with(vec![with_id(
        notes_clip(0, BAR, vec![note(0, 10, 60, 0.5)]),
        1,
    )]))]);
    assert!(flatten_notes(&all_muted).is_empty());
}

#[test]
fn flatten_notes_clips_of_different_lanes_do_not_hide_each_other() {
    // On one lane the later clip would hide the earlier one over [400, 600).
    let a = with_id(notes_clip(0, 1000, vec![note(500, 10, 61, 0.5)]), 1);
    let b = with_id(notes_clip(400, 200, vec![note(0, 10, 70, 0.5)]), 2);
    let t = track_of(vec![lane_with(vec![a]), lane_with(vec![b])]);
    assert_eq!(flatten_notes(&t), [(400, 10, 70, 0.5), (500, 10, 61, 0.5)]);
}

#[test]
fn flatten_notes_equal_tick_and_key_keep_the_lane_order() {
    let a = with_id(notes_clip(0, BAR, vec![note(0, 10, 60, 0.25)]), 1);
    let b = with_id(notes_clip(0, BAR, vec![note(0, 20, 60, 0.75)]), 2);
    let t = track_of(vec![lane_with(vec![a]), lane_with(vec![b])]);
    assert_eq!(flatten_notes(&t), [(0, 10, 60, 0.25), (0, 20, 60, 0.75)]);
}

#[test]
fn flatten_notes_ignores_the_automation_lane_between_and_after() {
    let a = with_id(notes_clip(0, BAR, vec![note(0, 10, 60, 0.5)]), 1);
    let b = with_id(notes_clip(0, BAR, vec![note(10, 10, 62, 0.5)]), 2);
    let t = track_of(vec![
        lane_with(vec![a]),
        lane_with(vec![b]),
        Lane::automation("filter.cutoff"),
    ]);
    assert_eq!(flatten_notes(&t).len(), 2);
    assert!(flatten_automation(&t, 2).is_empty());
}

// ---- controllers ----------------------------------------------------------------------------

#[test]
fn flatten_controllers_merges_lanes_per_kind_by_tick() {
    let a = with_controllers(
        with_id(notes_clip(0, BAR, vec![]), 1),
        vec![cc(1, vec![(0, 0.1), (200, 0.3)])],
    );
    let b = with_controllers(
        with_id(notes_clip(0, BAR, vec![]), 2),
        vec![cc(1, vec![(100, 0.2)]), cc(7, vec![(0, 0.9)])],
    );
    let t = track_of(vec![lane_with(vec![a]), lane_with(vec![b])]);
    let flat = flatten_controllers(&t);
    assert_eq!(flat.len(), 2);
    assert_eq!(flat[0].0, ControllerKind::Cc(1));
    assert_eq!(
        flat[0].1,
        [
            (0, 0.1, Curve::Step),
            (100, 0.2, Curve::Step),
            (200, 0.3, Curve::Step)
        ]
    );
    assert_eq!(flat[1].0, ControllerKind::Cc(7));
}

#[test]
fn flatten_controllers_at_an_equal_tick_the_later_lane_wins() {
    let a = with_controllers(
        with_id(notes_clip(0, BAR, vec![]), 1),
        vec![cc(1, vec![(100, 0.1)])],
    );
    let b = with_controllers(
        with_id(notes_clip(0, BAR, vec![]), 2),
        vec![cc(1, vec![(100, 0.9)])],
    );
    let t = track_of(vec![lane_with(vec![a]), lane_with(vec![b])]);
    let flat = flatten_controllers(&t);
    assert_eq!(flat[0].1, [(100, 0.9, Curve::Step)]);
}

#[test]
fn flatten_controllers_a_muted_lane_contributes_nothing() {
    let a = with_controllers(
        with_id(notes_clip(0, BAR, vec![]), 1),
        vec![cc(1, vec![(0, 0.1)])],
    );
    let b = with_controllers(
        with_id(notes_clip(0, BAR, vec![]), 2),
        vec![cc(2, vec![(0, 0.9)])],
    );
    let t = track_of(vec![lane_with(vec![a]), muted(lane_with(vec![b]))]);
    let flat = flatten_controllers(&t);
    assert_eq!(flat.len(), 1);
    assert_eq!(flat[0].0, ControllerKind::Cc(1));
}

// ---- properties -----------------------------------------------------------------------------

/// A note as `(tick, length, key)`.
type RawNote = (i64, i64, u8);

/// The notes of one bar's clip, its optional loop length and the lane it goes to.
type BarClip = (Vec<RawNote>, Option<i64>, usize);

/// One clip per bar (so no two overlap), each with random notes and an optional loop.
fn bar_clips() -> impl Strategy<Value = Vec<BarClip>> {
    prop::collection::vec(
        (
            prop::collection::vec((0i64..BAR, 1i64..2 * BAR, 0u8..128), 0..5),
            proptest::option::of(PPQ..BAR),
            0usize..6,
        ),
        1..8,
    )
}

fn clip_of_bar(bar: usize, notes: &[RawNote], loop_length: Option<i64>) -> Clip {
    let mut ns: Vec<Note> = notes.iter().map(|&(t, l, k)| note(t, l, k, 0.5)).collect();
    ns.sort_by_key(|n| (n.tick, n.key));
    let mut c = with_id(notes_clip(bar as i64 * BAR, BAR, ns), bar as u64 + 1);
    c.loop_length = loop_length;
    c
}

proptest! {
    /// Splitting the clips of one lane across several lanes does not change the notes that play
    /// when the clips do not overlap, whichever lane each clip is on.
    #[test]
    fn splitting_non_overlapping_clips_across_lanes_flattens_identically(
        clips in bar_clips(),
        n in 1usize..6,
    ) {
        let all: Vec<Clip> = clips
            .iter()
            .enumerate()
            .map(|(bar, (notes, lp, _))| clip_of_bar(bar, notes, *lp))
            .collect();
        let one = track_of(vec![lane_with(all.clone())]);
        let mut lanes: Vec<Lane> = (0..n).map(|_| Lane::notes()).collect();
        for (c, (_, _, pick)) in all.into_iter().zip(&clips) {
            lanes[pick % n].clips.push(c);
        }
        let many = track_of(lanes);
        prop_assert_eq!(flatten_notes(&one), flatten_notes(&many));
    }
}
