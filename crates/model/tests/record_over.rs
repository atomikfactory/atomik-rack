//! Recording over existing clips: `LayerRecording`, `EngulfRecording`, the loop-pass merge,
//! `GroupAsTakes` / `AddTake` for audio, and note tails (project format 35).

mod common;

use atomik_model::song::{audible_spans, flatten_notes, flatten_notes_with};
use atomik_model::{
    Clip, ClipContent, Command, ControllerKind, ControllerLane, Curve, Document, ModelError,
    PROJECT_VERSION, SongCommand, WorkflowCommand, covering_note_clip, empty_doc, layered_clip,
    load_project, merge_laps, new_project_doc, save_project,
};
use common::audio::*;
use common::song::*;
use common::workflow::*;
use common::*;

fn lead(reg: &atomik_device_sdk::Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    d
}

fn lane_clips(d: &Document) -> Vec<Clip> {
    d.doc().song.tracks[0].lanes[0].clips.clone()
}

fn notes_of(c: &Clip) -> Vec<(i64, i64, u8)> {
    match &c.content {
        ClipContent::Notes(n) => n.iter().map(|n| (n.tick, n.length, n.key)).collect(),
        _ => panic!("note clip"),
    }
}

fn engulf(clip: Clip) -> Command {
    wf(WorkflowCommand::EngulfRecording {
        device: key("lead"),
        lane: 0,
        clip,
    })
}

fn layer(into: atomik_model::ClipId, take: Clip) -> Command {
    wf(WorkflowCommand::LayerRecording { clip: into, take })
}

// ---- the incidental masking -----------------------------------------------------------------

/// A take that starts before an older clip is hidden by it from the older clip's start, even
/// where the older clip has no notes (the rule is by start, not by content): documented here,
/// and fixed by the Engulf and Layer recordings below.
#[test]
fn a_plain_take_starting_before_an_older_clip_is_masked_by_it() {
    let reg = registry();
    let mut d = lead(&reg);
    // The old clip (bar 3, four bars) has no notes at all.
    d.apply(
        add_clip("lead", 0, notes_clip(2 * BAR, 4 * BAR, vec![])),
        &reg,
    )
    .unwrap();
    // The take: bars 1-3 (three bars), a note at every bar.
    let take = notes_clip(
        0,
        3 * BAR,
        vec![
            note(0, 100, 60, 0.8),
            note(BAR, 100, 61, 0.8),
            note(2 * BAR, 100, 62, 0.8),
        ],
    );
    d.apply(add_clip("lead", 0, take), &reg).unwrap();
    let keys: Vec<u8> = flatten_notes(&d.doc().song.tracks[0])
        .into_iter()
        .map(|n| n.2)
        .collect();
    assert_eq!(
        keys,
        [60, 61],
        "the note at bar 3 is hidden by the empty older clip"
    );
}

// ---- Engulf ---------------------------------------------------------------------------------

#[test]
fn engulf_trims_an_older_clip_that_starts_inside_the_take_and_the_whole_take_plays() {
    let reg = registry();
    let mut d = lead(&reg);
    let old = notes_clip(
        2 * BAR,
        4 * BAR,
        vec![
            note(0, 100, 70, 0.8),
            note(BAR, 100, 71, 0.8),
            note(2 * BAR, 100, 72, 0.8),
        ],
    );
    d.apply(add_clip("lead", 0, old), &reg).unwrap();
    let before = save_project(d.doc());
    let take = notes_clip(
        0,
        3 * BAR,
        vec![
            note(0, 100, 60, 0.8),
            note(BAR, 100, 61, 0.8),
            note(2 * BAR, 100, 62, 0.8),
        ],
    );
    let ops = one_step(&mut d, &reg, engulf(take));
    assert!(!ops.is_empty());
    let clips = lane_clips(&d);
    assert_eq!(clips.len(), 2);
    let trimmed = clips.iter().find(|c| c.start > 0).unwrap();
    assert_eq!(
        (trimmed.start, trimmed.length, trimmed.offset),
        (3 * BAR, 3 * BAR, BAR)
    );
    let played: Vec<(i64, u8)> = flatten_notes(&d.doc().song.tracks[0])
        .into_iter()
        .map(|n| (n.0, n.2))
        .collect();
    // The whole take plays; the old clip's note under the take (content tick 0, at bar 3) is
    // gone; its notes from the take's end on (bars 4 and 5) still play.
    assert_eq!(
        played,
        [
            (0, 60),
            (BAR, 61),
            (2 * BAR, 62),
            (3 * BAR, 71),
            (4 * BAR, 72)
        ]
    );
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
}

#[test]
fn engulf_removes_an_older_clip_that_ends_inside_the_take_and_leaves_earlier_clips_alone() {
    let reg = registry();
    let mut d = lead(&reg);
    d.apply(
        add_clip(
            "lead",
            0,
            notes_clip(0, 2 * BAR, vec![note(0, 10, 50, 0.5)]),
        ),
        &reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "lead",
            0,
            notes_clip(3 * BAR, BAR, vec![note(0, 10, 51, 0.5)]),
        ),
        &reg,
    )
    .unwrap();
    // The take covers bars 2-6 (starts after the first clip, so it is not touched).
    let take = notes_clip(BAR, 5 * BAR, vec![note(0, 10, 60, 0.8)]);
    d.apply(engulf(take), &reg).unwrap();
    let starts: Vec<(i64, i64)> = lane_clips(&d).iter().map(|c| (c.start, c.length)).collect();
    assert_eq!(starts, [(0, 2 * BAR), (BAR, 5 * BAR)]);
}

#[test]
fn engulf_keeps_the_phase_of_a_looped_clip_it_trims() {
    let reg = registry();
    let mut d = lead(&reg);
    let mut looped = notes_clip(BAR, 6 * BAR, vec![note(0, 10, 70, 0.8)]);
    looped.loop_length = Some(BAR);
    d.apply(add_clip("lead", 0, looped), &reg).unwrap();
    let take = notes_clip(0, 2 * BAR + BAR / 2, vec![note(0, 10, 60, 0.8)]);
    d.apply(engulf(take), &reg).unwrap();
    let trimmed = lane_clips(&d)
        .into_iter()
        .find(|c| c.loop_length.is_some())
        .unwrap();
    assert_eq!(trimmed.start, 2 * BAR + BAR / 2);
    assert_eq!(trimmed.offset, BAR / 2, "mid-loop phase is kept");
    assert_eq!(trimmed.end(), 7 * BAR);
}

// ---- Layer ----------------------------------------------------------------------------------

#[test]
fn layer_merges_a_take_into_the_clip_under_its_start_and_keeps_the_old_notes() {
    let reg = registry();
    let mut d = lead(&reg);
    let old = notes_clip(
        0,
        2 * BAR,
        vec![note(0, 100, 60, 0.5), note(BAR, 200, 62, 0.5)],
    );
    d.apply(add_clip("lead", 0, old), &reg).unwrap();
    let id = last(&d);
    let before = save_project(d.doc());
    // The take starts at bar 2 (inside), runs three bars (past the clip end) and replays key 62
    // at the same start as the old note.
    let take = notes_clip(
        BAR,
        3 * BAR,
        vec![note(0, 50, 62, 0.9), note(2 * BAR, 100, 64, 0.8)],
    );
    one_step(&mut d, &reg, layer(id, take));
    let c = lane_clips(&d).remove(0);
    assert_eq!((c.id, c.start, c.length), (id, 0, 4 * BAR));
    assert_eq!(
        notes_of(&c),
        [(0, 100, 60), (BAR, 50, 62), (3 * BAR, 100, 64)],
        "old note kept, same start and key replaced, new note added"
    );
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
}

#[test]
fn layer_moves_the_song_end_out_when_the_clip_grows_past_it() {
    let reg = registry();
    let mut d = lead(&reg);
    d.apply(
        add_clip("lead", 0, notes_clip(0, BAR, vec![note(0, 10, 60, 0.5)])),
        &reg,
    )
    .unwrap();
    let id = last(&d);
    let end = d.doc().song.end;
    d.apply(
        layer(id, notes_clip(0, end + BAR, vec![note(0, 10, 61, 0.5)])),
        &reg,
    )
    .unwrap();
    assert!(d.doc().song.end >= end + BAR);
}

#[test]
fn layer_rejects_a_looped_clip_a_take_outside_it_and_audio() {
    let reg = registry();
    let mut d = lead(&reg);
    let mut looped = notes_clip(0, 2 * BAR, vec![note(0, 10, 60, 0.5)]);
    looped.loop_length = Some(BAR);
    d.apply(add_clip("lead", 0, looped), &reg).unwrap();
    let a = last(&d);
    d.apply(add_clip("lead", 0, notes_clip(4 * BAR, BAR, vec![])), &reg)
        .unwrap();
    let b = last(&d);
    let take = notes_clip(0, BAR, vec![note(0, 10, 61, 0.5)]);
    let e = rejected(&mut d, &reg, layer(a, take.clone()));
    assert!(matches!(e, ModelError::BadSong(_)), "{e}");
    rejected(&mut d, &reg, layer(b, take));
    rejected(
        &mut d,
        &reg,
        layer(atomik_model::ClipId(999), notes_clip(0, BAR, vec![])),
    );
}

#[test]
fn layer_replaces_the_controller_points_of_a_kind_over_the_take_and_keeps_other_kinds() {
    let pb = |points| ControllerLane::new(ControllerKind::PitchBend, points);
    let cc = |points| ControllerLane::new(ControllerKind::Cc(1), points);
    let mut old = notes_clip(0, 4 * BAR, vec![note(0, 10, 60, 0.5)]);
    old.controllers = vec![
        pb(vec![
            point(0, 0.1, Curve::Step),
            point(BAR + 10, 0.2, Curve::Step),
            point(3 * BAR, 0.3, Curve::Step),
        ]),
        cc(vec![point(0, 0.5, Curve::Step)]),
    ];
    let mut take = notes_clip(BAR, BAR, vec![note(0, 10, 61, 0.5)]);
    take.controllers = vec![pb(vec![point(5, -0.5, Curve::Step)])];
    let m = layered_clip(&old, &take).unwrap();
    assert_eq!(
        m.controllers[0],
        pb(vec![
            point(0, 0.1, Curve::Step),
            point(BAR + 5, -0.5, Curve::Step),
            point(3 * BAR, 0.3, Curve::Step)
        ]),
        "the old point inside the take's span is replaced"
    );
    assert_eq!(m.controllers[1], cc(vec![point(0, 0.5, Curve::Step)]));
}

// ---- passes of a loop take --------------------------------------------------------------------

#[test]
fn merge_laps_makes_one_clip_with_every_pass_and_a_later_pass_wins_a_tie() {
    let lap = |keys: &[(i64, u8)], vel: f32| {
        notes_clip(
            BAR,
            2 * BAR,
            keys.iter().map(|&(t, k)| note(t, 100, k, vel)).collect(),
        )
    };
    let laps = [
        lap(&[(0, 60), (PPQ, 62)], 0.5),
        lap(&[(PPQ, 62), (2 * PPQ, 64)], 0.9),
        lap(&[(3 * PPQ, 65)], 0.7),
    ];
    let m = merge_laps(&laps).unwrap();
    assert_eq!((m.start, m.length), (BAR, 2 * BAR));
    assert_eq!(
        notes_of(&m),
        [
            (0, 100, 60),
            (PPQ, 100, 62),
            (2 * PPQ, 100, 64),
            (3 * PPQ, 100, 65)
        ]
    );
    let ClipContent::Notes(n) = &m.content else {
        panic!()
    };
    assert_eq!(n[1].velocity, 0.9, "the later pass won the tie");
    assert!(merge_laps(&[]).is_none());
}

#[test]
fn merge_laps_spans_passes_that_start_at_different_ticks() {
    let a = notes_clip(0, 2 * BAR, vec![note(0, 10, 60, 0.5)]);
    let b = notes_clip(BAR, 2 * BAR, vec![note(0, 10, 61, 0.5)]);
    let m = merge_laps(&[a, b]).unwrap();
    assert_eq!((m.start, m.length), (0, 3 * BAR));
    assert_eq!(notes_of(&m), [(0, 10, 60), (BAR, 10, 61)]);
}

#[test]
fn covering_note_clip_picks_the_last_starting_unmuted_unlooped_clip() {
    let reg = registry();
    let mut d = lead(&reg);
    d.apply(add_clip("lead", 0, notes_clip(0, 4 * BAR, vec![])), &reg)
        .unwrap();
    let a = last(&d);
    d.apply(add_clip("lead", 0, notes_clip(BAR, 2 * BAR, vec![])), &reg)
        .unwrap();
    let b = last(&d);
    let lane = &d.doc().song.tracks[0].lanes[0];
    assert_eq!(covering_note_clip(lane, 0), Some(a));
    assert_eq!(covering_note_clip(lane, BAR + 1), Some(b));
    assert_eq!(covering_note_clip(lane, 3 * BAR + 1), Some(a));
    assert_eq!(covering_note_clip(lane, 4 * BAR), None);
}

// ---- audio: takes over an existing clip --------------------------------------------------------

fn group_count(d: &Document) -> usize {
    d.doc().song.take_groups().count()
}

#[test]
fn group_as_takes_stacks_a_new_audio_clip_over_the_old_one_and_plays_it_over_the_overlap() {
    let reg = audio_registry_no_console();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, 4 * BAR, 1), &reg)
        .unwrap();
    let old = last(&d);
    let before = save_project(d.doc());
    let new = clip(BAR, BAR, ClipContent::Audio(audio(2)));
    one_step(
        &mut d,
        &reg,
        sc(SongCommand::GroupAsTakes {
            device: key("audio-track"),
            lane: 0,
            clips: vec![old],
            extra: vec![new],
        }),
    );
    assert_eq!(group_count(&d), 1);
    let (_, _, g) = d
        .doc()
        .song
        .take_groups()
        .next()
        .map(|(k, l, g)| (k, l, g.clone()))
        .unwrap();
    assert_eq!(g.takes.len(), 2);
    assert_eq!(g.take_at(0), Some(0));
    assert_eq!(
        g.take_at(BAR),
        Some(1),
        "the new take plays over the overlap"
    );
    assert_eq!(
        g.take_at(2 * BAR),
        Some(0),
        "the old one plays where the new one is not"
    );
    // The old clip is no longer a plain lane clip: only the compiled clips are.
    let lane = &d.doc().song.tracks[0].lanes[0];
    assert!(lane.clips.iter().all(|c| c.id != old));
    assert_eq!(lane.clips.len(), 3);
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
    assert_eq!(group_count(&d), 0);
}

#[test]
fn group_as_takes_starting_before_the_old_clip_still_plays_the_new_take_over_the_overlap() {
    let reg = audio_registry_no_console();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 2 * BAR, 4 * BAR, 1), &reg)
        .unwrap();
    let old = last(&d);
    let new = clip(0, 3 * BAR, ClipContent::Audio(audio(2)));
    d.apply(
        sc(SongCommand::GroupAsTakes {
            device: key("audio-track"),
            lane: 0,
            clips: vec![old],
            extra: vec![new],
        }),
        &reg,
    )
    .unwrap();
    let g = d.doc().song.take_groups().next().unwrap().2.clone();
    assert_eq!(g.take_at(0), Some(1));
    assert_eq!(g.take_at(2 * BAR), Some(1));
    assert_eq!(g.take_at(3 * BAR), Some(0));
}

#[test]
fn add_take_puts_a_third_take_on_top_of_an_existing_group() {
    let reg = audio_registry_no_console();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, 4 * BAR, 1), &reg)
        .unwrap();
    let old = last(&d);
    d.apply(
        sc(SongCommand::GroupAsTakes {
            device: key("audio-track"),
            lane: 0,
            clips: vec![old],
            extra: vec![clip(BAR, BAR, ClipContent::Audio(audio(2)))],
        }),
        &reg,
    )
    .unwrap();
    let before = save_project(d.doc());
    one_step(
        &mut d,
        &reg,
        sc(SongCommand::AddTake {
            group: 1,
            clip: clip(BAR + BAR / 2, BAR, ClipContent::Audio(audio(1))),
        }),
    );
    let g = d.doc().song.take_groups().next().unwrap().2.clone();
    assert_eq!(g.takes.len(), 3);
    assert_eq!(g.take_at(BAR), Some(1));
    assert_eq!(g.take_at(BAR + BAR / 2), Some(2));
    assert_eq!(g.take_at(3 * BAR), Some(0));
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
}

#[test]
fn group_as_takes_rejects_a_clip_off_the_lane_a_compiled_clip_and_nothing() {
    let reg = audio_registry_no_console();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, 4 * BAR, 1), &reg)
        .unwrap();
    let old = last(&d);
    let cmd = |clips: Vec<atomik_model::ClipId>, extra: Vec<Clip>| {
        sc(SongCommand::GroupAsTakes {
            device: key("audio-track"),
            lane: 0,
            clips,
            extra,
        })
    };
    rejected(&mut d, &reg, cmd(vec![], vec![]));
    rejected(&mut d, &reg, cmd(vec![atomik_model::ClipId(99)], vec![]));
    d.apply(cmd(vec![old], vec![]), &reg).unwrap();
    let compiled = d.doc().song.tracks[0].lanes[0].clips[0].id;
    rejected(&mut d, &reg, cmd(vec![compiled], vec![]));
    rejected(
        &mut d,
        &reg,
        cmd(
            vec![],
            vec![clip(0, BAR, ClipContent::Notes(vec![].into()))],
        ),
    );
}

// ---- tails and the format ---------------------------------------------------------------------

fn tail_doc(reg: &atomik_device_sdk::Registry, tails: bool) -> Document {
    let mut d = lead(reg);
    // A one-bar clip whose note rings for two bars; a second clip starts at bar 2 and covers
    // the end of the long note.
    d.apply(
        add_clip(
            "lead",
            0,
            notes_clip(0, BAR, vec![note(0, 2 * BAR, 60, 0.8)]),
        ),
        reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "lead",
            0,
            notes_clip(BAR + BAR / 2, BAR, vec![note(0, 10, 61, 0.8)]),
        ),
        reg,
    )
    .unwrap();
    let mut doc = d.doc().clone();
    doc.song_mut().tails = tails;
    Document::new(doc)
}

#[test]
fn a_note_is_cut_at_its_piece_end_without_tails_and_rings_out_with_them() {
    let reg = registry();
    let off = tail_doc(&reg, false);
    let on = tail_doc(&reg, true);
    let cut = flatten_notes(&off.doc().song.tracks[0]);
    assert_eq!(cut[0], (0, BAR, 60, 0.8), "legacy: cut at the clip end");
    assert_eq!(
        flatten_notes_with(&off.doc().song.tracks[0], false),
        cut,
        "flatten_notes is the legacy rule"
    );
    let full = flatten_notes_with(&on.doc().song.tracks[0], true);
    assert_eq!(
        full[0],
        (0, 2 * BAR, 60, 0.8),
        "tails: the note's own length"
    );
    // Only starts are masked: a note that starts under a later clip is still dropped.
    let mut under = tail_doc(&reg, true).doc().clone();
    under.song_mut().tracks[0].lanes[0].clips[0].length = 2 * BAR;
    under.song_mut().tracks[0].lanes[0].clips[0].content =
        ClipContent::Notes(vec![note(0, 100, 60, 0.8), note(BAR + BAR / 2, 100, 62, 0.8)].into());
    let flat = flatten_notes_with(&under.song.tracks[0], true);
    assert!(
        flat.iter().all(|n| n.2 != 62),
        "a note starting under a later clip is masked"
    );
    // Audible spans are the same either way.
    assert_eq!(audible_spans(&off.doc().song.tracks[0].lanes[0]).len(), 2);
}

#[test]
fn tails_are_saved_only_when_on_and_a_new_project_has_them() {
    let reg = registry();
    assert!(!empty_doc().song.tails);
    assert!(new_project_doc().song.tails);
    let off = save_project(&empty_doc());
    assert!(!off.contains("\"tails\""));
    let mut doc = empty_doc();
    doc.song_mut().tails = true;
    let on = save_project(&doc);
    assert!(on.contains("\"tails\": true,"), "{on}");
    assert!(on.contains(&format!("\"version\": {PROJECT_VERSION}")));
    assert!(load_project(&on, &reg).unwrap().song.tails);
    assert_eq!(save_project(&load_project(&on, &reg).unwrap()), on);
    assert!(!load_project(&off, &reg).unwrap().song.tails);
}

#[test]
fn a_version_34_file_loads_without_tails_and_resaves_as_35_with_the_version_line_alone_changed() {
    let reg = registry();
    let text = save_project(&empty_doc());
    let v34 = text.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 34",
        1,
    );
    let doc = load_project(&v34, &reg).unwrap();
    assert!(!doc.song.tails);
    assert_eq!(save_project(&doc), text);
}

#[test]
fn tails_need_version_35_and_must_be_true() {
    let reg = registry();
    let mut doc = empty_doc();
    doc.song_mut().tails = true;
    let text = save_project(&doc);
    let v34 = text.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 34",
        1,
    );
    let e = load_project(&v34, &reg).unwrap_err().to_string();
    assert!(e.contains("tails"), "{e}");
    let falsy = text.replacen("\"tails\": true", "\"tails\": false", 1);
    let e = load_project(&falsy, &reg).unwrap_err().to_string();
    assert!(e.contains("tails"), "{e}");
}

// ---- Document::apply_joined -------------------------------------------------------------------

#[test]
fn apply_joined_folds_a_follow_up_into_the_step_before_it() {
    let reg = registry();
    let mut d = lead(&reg);
    d.apply(
        add_clip("lead", 0, notes_clip(0, BAR, vec![note(0, 10, 60, 0.5)])),
        &reg,
    )
    .unwrap();
    let id = last(&d);
    let before = save_project(d.doc());
    d.apply(
        sc(SongCommand::SetClipContent {
            id,
            content: ClipContent::Notes(vec![note(0, 10, 60, 0.5), note(BAR, 10, 61, 0.5)].into()),
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    d.apply_joined(
        sc(SongCommand::SetClipGeometry {
            id,
            start: 0,
            length: 2 * BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(lane_clips(&d)[0].length, 2 * BAR);
    d.undo(&reg).unwrap();
    assert_eq!(
        save_project(d.doc()),
        before,
        "one undo reverts the edit and its follow-up"
    );
    d.redo(&reg).unwrap();
    assert_eq!(lane_clips(&d)[0].length, 2 * BAR);
}
