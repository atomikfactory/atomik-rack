//! `SetLinkedComp`: one comp gesture over the take groups of several tracks, as one undo step.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::takes::CompSegment;
use atomik_model::{Clip, Command, Document, ModelError, SongCommand, save_project};
use common::song::*;
use common::*;

fn seg(take: usize, start: i64, end: i64) -> CompSegment {
    CompSegment::new(take, start, end)
}

fn take(start: i64, length: i64, key: u8) -> Clip {
    notes_clip(start, length, vec![note(0, PPQ / 4, key, 0.8)])
}

fn song_cmd(c: SongCommand) -> Command {
    sc(c)
}

fn comps(d: &Document) -> Vec<Vec<CompSegment>> {
    d.doc()
        .song
        .take_groups()
        .map(|(_, _, g)| g.comp.clone())
        .collect()
}

/// Tracks "lead" and "pad" with a group of `takes` two-bar takes each (lead: group 1, pad: group 2).
fn two_tracks(reg: &Registry, a: usize, b: usize) -> Document {
    let mut d = song_start(reg);
    d.apply(add(4, SYNTH, Some("pad")), reg).unwrap();
    for (name, n) in [("lead", a), ("pad", b)] {
        d.apply(add_track(name), reg).unwrap();
        d.apply(
            song_cmd(SongCommand::NewTakeGroup {
                device: key(name),
                lane: 0,
                takes: (0..n).map(|i| take(0, 2 * BAR, 60 + i as u8)).collect(),
            }),
            reg,
        )
        .unwrap();
    }
    d
}

fn linked(groups: &[u32], take: usize, start: i64, end: i64) -> Command {
    song_cmd(SongCommand::SetLinkedComp {
        groups: groups.to_vec(),
        take,
        start,
        end,
    })
}

#[test]
fn one_gesture_combs_every_group_in_one_undo_step() {
    let reg = registry();
    let mut d = two_tracks(&reg, 3, 3);
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    d.apply(linked(&[1, 2], 1, 0, BAR), &reg).unwrap();
    assert_eq!(d.undo_depth(), depth + 1);
    for comp in comps(&d) {
        assert_eq!(comp[0], seg(1, 0, BAR), "the first bar plays take 1");
    }
    d.undo().unwrap();
    assert_eq!(save_project(d.doc()), before);
    d.redo().unwrap();
    assert_eq!(comps(&d)[0][0], seg(1, 0, BAR));
    assert_eq!(comps(&d)[1][0], seg(1, 0, BAR));
}

#[test]
fn compiled_clips_have_distinct_ids_across_the_groups() {
    let reg = registry();
    let mut d = two_tracks(&reg, 2, 2);
    d.apply(linked(&[1, 2], 0, 0, BAR), &reg).unwrap();
    let mut ids: Vec<_> = d
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter().flat_map(|l| l.clips.iter().map(|c| c.id)))
        .collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n);
}

#[test]
fn a_group_without_that_take_is_left_alone() {
    let reg = registry();
    let mut d = two_tracks(&reg, 3, 1);
    let before = comps(&d)[1].clone();
    d.apply(linked(&[1, 2], 1, 0, BAR), &reg).unwrap();
    assert_eq!(comps(&d)[0][0], seg(1, 0, BAR));
    assert_eq!(comps(&d)[1], before, "the one-take group did not change");
}

#[test]
fn the_range_is_cut_back_to_each_group() {
    let reg = registry();
    let mut d = two_tracks(&reg, 2, 2);
    d.apply(linked(&[1, 2], 0, BAR, 10 * BAR), &reg).unwrap();
    for comp in comps(&d) {
        assert_eq!(comp.last().unwrap().end, 2 * BAR);
    }
}

#[test]
fn bad_requests_are_refused_and_change_nothing() {
    let reg = registry();
    let mut d = two_tracks(&reg, 2, 2);
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    let cases = [
        linked(&[], 0, 0, BAR),
        linked(&[1, 1], 0, 0, BAR),
        linked(&[1, 99], 0, 0, BAR),
        linked(&[1, 2], 0, BAR, BAR),
        // No such take in any group.
        linked(&[1, 2], 7, 0, BAR),
        linked(&[1, 2], 0, 20 * BAR, 21 * BAR),
    ];
    for cmd in cases {
        let e: ModelError = d.apply(cmd, &reg).expect_err("must be refused");
        let _ = e;
        assert_eq!(save_project(d.doc()), before);
        assert_eq!(d.undo_depth(), depth);
    }
}

#[test]
fn a_comp_that_changes_nothing_is_refused() {
    let reg = registry();
    let mut d = two_tracks(&reg, 2, 2);
    // Layered takes of the same extent: the last take plays everywhere.
    assert!(d.apply(linked(&[1, 2], 1, 0, 2 * BAR), &reg).is_err());
}
