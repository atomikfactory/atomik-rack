//! Takes on automation lanes (a loop recording of automation stacks its passes as takes) and the
//! saved link between the take groups of several tracks.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    AutomationPoint, ClipContent, Command, Curve, Document, SongCommand, WorkflowCommand,
    load_project, save_project,
};
use common::song::*;
use common::*;

const CUTOFF: &str = "filter.cutoff";

fn song_cmd(c: SongCommand) -> Command {
    sc(c)
}

fn flat(from: f32, to: f32) -> Vec<AutomationPoint> {
    vec![point(0, from, Curve::Linear), point(BAR, to, Curve::Linear)]
}

fn pass(start: i64, from: f32, to: f32) -> atomik_model::Clip {
    auto_clip(start, BAR, flat(from, to))
}

/// Track `lead` with a cutoff lane (lane 1) holding a group of two passes over the first bar.
fn lead(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    d.apply(
        song_cmd(SongCommand::AddAutomationLane {
            device: key("lead"),
            param: CUTOFF.into(),
        }),
        reg,
    )
    .unwrap();
    d.apply(
        song_cmd(SongCommand::NewTakeGroup {
            device: key("lead"),
            lane: 1,
            takes: vec![pass(0, 100.0, 200.0), pass(0, 300.0, 400.0)],
        }),
        reg,
    )
    .unwrap();
    d
}

fn lane_points(d: &Document) -> Vec<Vec<f32>> {
    d.doc().song.tracks[0].lanes[1]
        .clips
        .iter()
        .map(|c| match &c.content {
            ClipContent::Automation(p) => p.iter().map(|p| p.value).collect(),
            _ => panic!("not an automation clip"),
        })
        .collect()
}

#[test]
fn passes_stack_as_takes_and_the_last_one_plays() {
    let reg = registry();
    let mut d = lead(&reg);
    assert_eq!(lane_points(&d), [vec![300.0, 400.0]]);
    let (_, lane, g) = d.doc().song.take_group(1).unwrap();
    assert_eq!((lane, g.takes.len()), (1, 2));
    d.apply(song_cmd(SongCommand::PickTake { group: 1, take: 0 }), &reg)
        .unwrap();
    assert_eq!(lane_points(&d), [vec![100.0, 200.0]]);
    d.apply(
        song_cmd(SongCommand::SetCompSegment {
            group: 1,
            take: 1,
            start: BAR / 2,
            end: BAR,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(
        lane_points(&d).len(),
        2,
        "a comp of two takes plays two clips"
    );
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!(lane_points(&d), [vec![300.0, 400.0]]);
}

#[test]
fn a_take_group_on_an_automation_lane_saves_and_loads() {
    let reg = registry();
    let d = lead(&reg);
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(save_project(&loaded), text);
    assert_eq!(loaded.song.take_groups().count(), 1);
    assert_eq!(loaded.song, d.doc().song);
}

#[test]
fn one_batch_makes_the_take_group_and_one_undo_removes_it() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    let before = d.doc().clone();
    d.apply(
        Command::Batch(vec![
            song_cmd(SongCommand::AddAutomationLane {
                device: key("lead"),
                param: CUTOFF.into(),
            }),
            song_cmd(SongCommand::NewTakeGroup {
                device: key("lead"),
                lane: 1,
                takes: vec![pass(0, 1.0, 2.0), pass(0, 3.0, 4.0)],
            }),
        ]),
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().song.take_groups().count(), 1);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn an_automation_take_must_be_an_automation_clip() {
    let reg = registry();
    let mut d = lead(&reg);
    let before = save_project(d.doc());
    let result = d.apply(
        song_cmd(SongCommand::AddTake {
            group: 1,
            clip: notes_clip(0, BAR, vec![note(0, PPQ / 4, 60, 0.8)]),
        }),
        &reg,
    );
    assert!(result.is_err());
    assert_eq!(save_project(d.doc()), before);
}

// ---- the saved link ---------------------------------------------------------------------

fn take(start: i64, key: u8) -> atomik_model::Clip {
    notes_clip(start, 2 * BAR, vec![note(0, PPQ / 4, key, 0.8)])
}

/// Tracks "lead", "pad" and "bass" with a group of two takes each (groups 1, 2 and 3).
fn three_tracks(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add(4, SYNTH, Some("pad")), reg).unwrap();
    d.apply(add(5, SYNTH, Some("bass")), reg).unwrap();
    for name in ["lead", "pad", "bass"] {
        d.apply(add_track(name), reg).unwrap();
        d.apply(
            song_cmd(SongCommand::NewTakeGroup {
                device: key(name),
                lane: 0,
                takes: vec![take(0, 60), take(0, 62)],
            }),
            reg,
        )
        .unwrap();
    }
    d
}

fn links(d: &Document) -> Vec<Option<u32>> {
    d.doc().song.take_groups().map(|(_, _, g)| g.link).collect()
}

fn link(groups: &[u32]) -> Command {
    song_cmd(SongCommand::LinkTakeGroups {
        groups: groups.to_vec(),
    })
}

#[test]
fn linking_shares_a_tag_in_one_undo_step() {
    let reg = registry();
    let mut d = three_tracks(&reg);
    assert_eq!(links(&d), [None, None, None]);
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    d.apply(link(&[1, 3]), &reg).unwrap();
    assert_eq!(d.undo_depth(), depth + 1);
    assert_eq!(links(&d), [Some(1), None, Some(1)]);
    d.undo().unwrap();
    assert_eq!(save_project(d.doc()), before);
    d.redo().unwrap();
    assert_eq!(links(&d), [Some(1), None, Some(1)]);
}

#[test]
fn a_group_linked_again_leaves_its_old_link() {
    let reg = registry();
    let mut d = three_tracks(&reg);
    d.apply(link(&[1, 2]), &reg).unwrap();
    d.apply(link(&[2, 3]), &reg).unwrap();
    assert_eq!(links(&d), [Some(1), Some(2), Some(2)]);
    d.apply(song_cmd(SongCommand::UnlinkTakeGroup { group: 3 }), &reg)
        .unwrap();
    assert_eq!(links(&d), [Some(1), Some(2), None]);
}

#[test]
fn bad_link_requests_are_refused_and_change_nothing() {
    let reg = registry();
    let mut d = three_tracks(&reg);
    d.apply(link(&[1, 2]), &reg).unwrap();
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    for bad in [
        link(&[1]),
        link(&[1, 1]),
        link(&[1, 9]),
        link(&[1, 2]),
        link(&(1..=65).collect::<Vec<_>>()),
        song_cmd(SongCommand::UnlinkTakeGroup { group: 3 }),
        song_cmd(SongCommand::UnlinkTakeGroup { group: 9 }),
    ] {
        assert!(d.apply(bad, &reg).is_err());
    }
    assert_eq!(save_project(d.doc()), before);
    assert_eq!(d.undo_depth(), depth);
}

#[test]
fn the_link_is_saved_only_when_set_and_round_trips() {
    let reg = registry();
    let mut d = three_tracks(&reg);
    let plain = save_project(d.doc());
    assert!(!plain.contains("\"link\""), "{plain}");
    d.apply(link(&[1, 3]), &reg).unwrap();
    let text = save_project(d.doc());
    assert_eq!(text.matches("\"link\": 1").count(), 2, "{text}");
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(save_project(&loaded), text);
    let tags: Vec<_> = loaded.song.take_groups().map(|(_, _, g)| g.link).collect();
    assert_eq!(tags, [Some(1), None, Some(1)]);
    // A file without the field loads unlinked, and a tag of 0 is refused.
    let loaded = load_project(&plain, &reg).unwrap();
    assert!(loaded.song.take_groups().all(|(_, _, g)| g.link.is_none()));
    assert!(load_project(&text.replacen("\"link\": 1", "\"link\": 0", 1), &reg).is_err());
}

#[test]
fn a_comp_keeps_the_link_and_a_copied_track_is_not_linked() {
    let reg = registry();
    let mut d = three_tracks(&reg);
    d.apply(link(&[1, 2]), &reg).unwrap();
    d.apply(song_cmd(SongCommand::PickTake { group: 1, take: 0 }), &reg)
        .unwrap();
    assert_eq!(links(&d), [Some(1), Some(1), None]);
    d.apply(
        sc_wf(WorkflowCommand::DuplicateTrack {
            device: key("lead"),
        }),
        &reg,
    )
    .unwrap();
    let copied: Vec<_> = d
        .doc()
        .song
        .take_groups()
        .filter(|(dev, _, _)| **dev != key("lead") && **dev != key("pad") && **dev != key("bass"))
        .map(|(_, _, g)| g.link)
        .collect();
    assert_eq!(copied, [None]);
}

fn sc_wf(c: WorkflowCommand) -> Command {
    Command::Workflow(c)
}
