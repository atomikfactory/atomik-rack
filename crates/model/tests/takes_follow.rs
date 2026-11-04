//! Takes clipboard (`TakeSnapshot`, `WorkflowCommand::PasteTakes`) and editing a take through the
//! commands of a plain clip (what the piano roll and the audio editor send): one undo step each,
//! a refusal changes nothing, and what plays follows.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::flatten_audio;
use atomik_model::takes::CompSegment;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, Document, ModelError, NoteList, SongCommand, TakeGroup,
    TakeSnapshot, TakeTarget, WorkflowCommand, save_project,
};
use common::audio::*;
use common::song::*;
use common::*;

fn take(start: i64, length: i64, key: u8) -> Clip {
    notes_clip(start, length, vec![note(0, PPQ / 4, key, 0.8)])
}

fn group_of(d: &Document, id: u32) -> TakeGroup {
    d.doc().song.take_group(id).expect("a group").2.clone()
}

fn wf(c: WorkflowCommand) -> Command {
    Command::Workflow(c)
}

/// Two instrument tracks: `lead` with a group of three takes (keys 60..62, two bars from 0) and
/// `synth` with a group of one take two bars long from bar 4.
fn two_groups(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    for name in ["lead", "synth"] {
        d.apply(add_track(name), reg).unwrap();
    }
    d.apply(
        sc(SongCommand::NewTakeGroup {
            device: key("lead"),
            lane: 0,
            takes: (0..3).map(|i| take(0, 2 * BAR, 60 + i)).collect(),
        }),
        reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::NewTakeGroup {
            device: key("synth"),
            lane: 0,
            takes: vec![take(4 * BAR, 2 * BAR, 36)],
        }),
        reg,
    )
    .unwrap();
    d
}

/// Applies `command` and checks it is one undo step that undoes and redoes exactly.
fn step(d: &mut Document, command: Command, reg: &Registry) {
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    d.apply(command, reg).unwrap();
    assert_eq!(d.undo_depth(), depth + 1);
    let after = save_project(d.doc());
    d.undo().unwrap();
    assert_eq!(save_project(d.doc()), before);
    d.redo().unwrap();
    assert_eq!(save_project(d.doc()), after);
}

fn rejects(d: &mut Document, command: Command, reg: &Registry) -> ModelError {
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    let e = d.apply(command, reg).expect_err("must be rejected");
    assert_eq!(save_project(d.doc()), before);
    assert_eq!(d.undo_depth(), depth);
    e
}

// ---- the takes clipboard ------------------------------------------------------------------

#[test]
fn copied_takes_keep_their_layout_relative_to_the_earliest() {
    let reg = registry();
    let d = two_groups(&reg);
    let s = TakeSnapshot::capture(d.doc(), 2, &[0]).unwrap();
    assert_eq!(s.takes.len(), 1);
    assert_eq!((s.takes[0].start, s.span), (0, 2 * BAR));
    assert_eq!(s.takes[0].id, ClipId(0));
    let s = TakeSnapshot::capture(d.doc(), 1, &[2, 0, 2]).unwrap();
    assert_eq!(s.takes.len(), 2, "sorted and without repeats");
    assert!(!s.is_audio());
    assert!(TakeSnapshot::capture(d.doc(), 1, &[]).is_err());
    assert!(TakeSnapshot::capture(d.doc(), 1, &[3]).is_err());
    assert!(TakeSnapshot::capture(d.doc(), 9, &[0]).is_err());
}

#[test]
fn takes_paste_into_another_group_on_top_and_play_over_their_extent() {
    let reg = registry();
    let mut d = two_groups(&reg);
    let s = TakeSnapshot::capture(d.doc(), 1, &[0, 1]).unwrap();
    let at = group_of(&d, 2).range().0;
    step(
        &mut d,
        wf(WorkflowCommand::PasteTakes {
            target: TakeTarget::Group(2),
            at_tick: at,
            takes: s,
        }),
        &reg,
    );
    let g = group_of(&d, 2);
    assert_eq!(g.takes.len(), 3);
    assert_eq!(
        g.takes[1].start,
        4 * BAR,
        "the earliest pasted take starts at the tick"
    );
    assert_ne!(g.takes[1].id, g.takes[2].id);
    // The last pasted take is on top and plays over the whole extent.
    assert_eq!(g.comp, [CompSegment::new(2, 4 * BAR, 6 * BAR)]);
    let lane = &d.doc().song.tracks[1].lanes[0];
    assert_eq!(lane.clips.len(), 1);
    let ClipContent::Notes(n) = &lane.clips[0].content else {
        panic!("notes")
    };
    assert_eq!(n[0].key, 61);
    // The source group is untouched.
    assert_eq!(group_of(&d, 1).takes.len(), 3);
}

#[test]
fn takes_paste_onto_a_lane_as_a_new_group_at_the_tick() {
    let reg = registry();
    let mut d = two_groups(&reg);
    d.apply(add(4, SYNTH, Some("pad")), &reg).unwrap();
    d.apply(add_track("pad"), &reg).unwrap();
    let s = TakeSnapshot::capture(d.doc(), 1, &[1]).unwrap();
    step(
        &mut d,
        wf(WorkflowCommand::PasteTakes {
            target: TakeTarget::Lane {
                device: key("pad"),
                lane: 0,
            },
            at_tick: 8 * BAR,
            takes: s,
        }),
        &reg,
    );
    let (t, _, g) = d
        .doc()
        .song
        .take_groups()
        .last()
        .map(|(k, l, g)| (k.clone(), l, g))
        .unwrap();
    assert_eq!(t, key("pad"));
    assert_eq!(g.takes.len(), 1);
    assert_eq!(g.range(), (8 * BAR, 10 * BAR));
}

#[test]
fn a_paste_of_the_wrong_kind_or_into_nothing_is_refused_and_changes_nothing() {
    let source = two_groups(&registry());
    let s = TakeSnapshot::capture(source.doc(), 1, &[0]).unwrap();
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    // Notes onto the audio lane.
    rejects(
        &mut d,
        wf(WorkflowCommand::PasteTakes {
            target: TakeTarget::Lane {
                device: key("audio-track"),
                lane: 0,
            },
            at_tick: 0,
            takes: s.clone(),
        }),
        &reg,
    );
    rejects(
        &mut d,
        wf(WorkflowCommand::PasteTakes {
            target: TakeTarget::Group(77),
            at_tick: 0,
            takes: s.clone(),
        }),
        &reg,
    );
    let empty = TakeSnapshot {
        takes: Vec::new(),
        ..s
    };
    rejects(
        &mut d,
        wf(WorkflowCommand::PasteTakes {
            target: TakeTarget::Group(1),
            at_tick: 0,
            takes: empty,
        }),
        &reg,
    );
}

/// An audio group of two takes (assets 1 and 2) on `audio-track`.
fn audio_group(reg: &Registry) -> Document {
    let mut d = with_audio_track(reg);
    let clip = |n: u8| Clip {
        content: ClipContent::Audio(audio(n)),
        ..notes_clip(0, 2 * BAR, Vec::new())
    };
    d.apply(
        sc(SongCommand::NewTakeGroup {
            device: key("audio-track"),
            lane: 0,
            takes: vec![clip(1), clip(2)],
        }),
        reg,
    )
    .unwrap();
    d
}

#[test]
fn audio_takes_paste_into_another_song_only_with_their_assets() {
    let reg = audio_registry();
    let from = audio_group(&reg);
    let s = TakeSnapshot::capture(from.doc(), 1, &[1]).unwrap();
    assert!(s.is_audio());
    assert_eq!(s.assets.len(), 1);
    // A song whose pool lacks the asset refuses the paste.
    let mut to = Document::new(atomik_model::empty_doc());
    to.apply(add_audio_track(), &reg).unwrap();
    let lane = TakeTarget::Lane {
        device: key("audio-track"),
        lane: 0,
    };
    let command = wf(WorkflowCommand::PasteTakes {
        target: lane,
        at_tick: BAR,
        takes: s.clone(),
    });
    rejects(&mut to, command.clone(), &reg);
    // The app registers the pool entries first (not an undo step), then it goes through.
    for (hash, meta) in s.assets.clone() {
        to.apply(Command::RegisterAsset { hash, meta }, &reg)
            .unwrap();
    }
    step(&mut to, command, &reg);
    let g = group_of(&to, 1);
    assert_eq!(g.takes.len(), 1);
    assert_eq!(g.range(), (BAR, 3 * BAR));
    let regions = flatten_audio(&to.doc().song.tracks[0], &to.doc().song, 48_000);
    assert_eq!(regions.len(), 1);
}

// ---- editing a take -------------------------------------------------------------------------

#[test]
fn a_clip_command_on_a_take_edits_the_take_and_the_lane_plays_it() {
    let reg = registry();
    let mut d = two_groups(&reg);
    let id = group_of(&d, 1).takes[2].id;
    assert!(d.doc().song.clip(id).is_none(), "a take is not a lane clip");
    assert!(d.doc().song.clip_or_take(id).is_some());
    let notes: NoteList = vec![note(0, PPQ, 72, 0.5), note(PPQ, PPQ, 74, 0.5)].into();
    step(
        &mut d,
        sc(SongCommand::SetClipContent {
            id,
            content: ClipContent::Notes(notes.clone()),
            merge: None,
        }),
        &reg,
    );
    let g = group_of(&d, 1);
    assert_eq!(g.takes[2].id, id, "the take keeps its id");
    assert_eq!(g.takes[2].content, ClipContent::Notes(notes.clone()));
    // Take 2 is on top, so the compiled lane clip carries the edit.
    let lane = &d.doc().song.tracks[0].lanes[0];
    assert_eq!(lane.clips.len(), 1);
    assert_eq!(lane.clips[0].content, ClipContent::Notes(notes));
    // Scale and controller commands reach takes too.
    step(
        &mut d,
        sc(SongCommand::SetClipControllers {
            clip: id,
            lanes: Vec::new(),
            merge: None,
        }),
        &reg,
    );
}

#[test]
fn an_audio_take_edit_through_set_audio_clip_moves_the_take() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    let g = group_of(&d, 1);
    let id = g.takes[1].id;
    let mut content = audio(2);
    content.gain_db = -6.0;
    step(
        &mut d,
        sc(SongCommand::SetAudioClip {
            id,
            start: 0,
            length: 2 * BAR,
            muted: false,
            content: content.clone(),
            merge: None,
        }),
        &reg,
    );
    assert_eq!(
        group_of(&d, 1).takes[1].content,
        ClipContent::Audio(content)
    );
}

#[test]
fn a_command_a_take_cannot_take_is_refused_and_changes_nothing() {
    let reg = registry();
    let mut d = two_groups(&reg);
    let id = group_of(&d, 1).takes[0].id;
    // Audio edits on a note take, an unknown clip and a command that is not an edit.
    let e = rejects(
        &mut d,
        sc(SongCommand::SetWarpMarkers {
            id,
            markers: Vec::new(),
            merge: None,
        }),
        &reg,
    );
    assert!(!e.to_string().is_empty());
    rejects(
        &mut d,
        sc(SongCommand::SetClipContent {
            id: ClipId(9999),
            content: ClipContent::Notes(NoteList::new()),
            merge: None,
        }),
        &reg,
    );
    rejects(&mut d, sc(SongCommand::SplitClip { id, at: BAR }), &reg);
}

#[test]
fn notes_paste_into_a_take_the_piano_roll_has_open() {
    let reg = registry();
    let mut d = two_groups(&reg);
    let id = group_of(&d, 1).takes[2].id;
    let before = group_of(&d, 1).takes[2].clone();
    let ClipContent::Notes(old) = &before.content else {
        panic!("notes")
    };
    let snapshot = atomik_model::NoteSnapshot {
        notes: vec![note(0, PPQ, 80, 0.7)],
        span: PPQ,
    };
    step(
        &mut d,
        wf(WorkflowCommand::PasteNotes {
            clip: id,
            at_tick: PPQ,
            notes: snapshot,
        }),
        &reg,
    );
    let ClipContent::Notes(now) = &group_of(&d, 1).takes[2].content else {
        panic!("notes")
    };
    assert_eq!(now.len(), old.len() + 1);
    assert!(now.iter().any(|n| n.key == 80));
    // The lane plays the top take, so the compiled clip has the note too.
    let ClipContent::Notes(played) = &d.doc().song.tracks[0].lanes[0].clips[0].content else {
        panic!("notes")
    };
    assert!(played.iter().any(|n| n.key == 80));
}
