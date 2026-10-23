//! `Command::Song`: one test per song command, every rejection, undo, drag-merge and
//! the interaction with rack commands.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipContent, ClipId, Command, Curve, Document, Lane, LaneKind, MeterEvent, ModelError,
    SongCommand, SyncOp, TempoEvent, empty_song,
};
use common::song::*;
use common::*;

fn rejects(d: &mut Document, cmd: Command, reg: &Registry) -> ModelError {
    let before = d.doc().clone();
    let undo = d.can_undo();
    let last = d.last_clip_id();
    let e = d.apply(cmd, reg).expect_err("command must be rejected");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
    assert_eq!(d.can_undo(), undo, "a rejected command records no step");
    assert_eq!(d.last_clip_id(), last);
    e
}

/// chain + `lead` synth, with a track for `synth`.
fn with_track(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    Document::new(d.doc().clone())
}

fn changed() -> Vec<SyncOp> {
    vec![SyncOp::SongChanged]
}

fn undo_all_restores(d: &mut Document, before: &atomik_model::RackDoc, reg: &Registry) {
    while d.undo(reg).is_some() {}
    assert_eq!(d.doc(), before);
}

// ---- tracks ---------------------------------------------------------------------------------

#[test]
fn add_track_creates_note_lane() {
    let reg = registry();
    let mut d = song_start(&reg);
    let before = d.doc().clone();
    assert_eq!(d.apply(add_track("synth"), &reg).unwrap(), changed());
    assert_eq!(d.apply(add_track("lead"), &reg).unwrap(), changed());
    let t = &d.doc().song.tracks;
    assert_eq!(t.len(), 2);
    assert_eq!(
        (t[0].device.as_str(), t[1].device.as_str()),
        ("synth", "lead")
    );
    assert!(!t[0].muted && !t[0].solo && !t[0].armed);
    assert_eq!(
        t[0].lanes,
        [Lane {
            kind: LaneKind::Notes,
            clips: vec![]
        }]
    );
    assert_eq!(
        d.doc().song.track(&key("lead")).unwrap().device,
        key("lead")
    );
    assert!(d.doc().song.track(&key("delay")).is_none());
    let ops = d.undo(&reg).unwrap();
    assert_eq!(ops, changed());
    undo_all_restores(&mut d, &before, &reg);
}

#[test]
fn add_track_rejections() {
    let reg = registry();
    let mut d = with_track(&reg);
    assert_eq!(
        rejects(&mut d, add_track("nope"), &reg),
        ModelError::UnknownDevice("nope".into())
    );
    assert!(matches!(
        rejects(&mut d, add_track("delay"), &reg),
        ModelError::BadSong(m) if m.contains("Notes input")
    ));
    assert!(matches!(
        rejects(&mut d, add_track("synth"), &reg),
        ModelError::BadSong(m) if m.contains("already")
    ));
}

#[test]
fn add_device_does_not_create_a_track() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add(0, SYNTH, Some("extra")), &reg).unwrap();
    assert!(d.doc().song.tracks.is_empty());
}

#[test]
fn remove_track_and_undo_restores_it_at_its_index_with_clips() {
    let reg = registry();
    let mut d = with_track(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(
        add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, 100, 60, 0.5)])),
        &reg,
    )
    .unwrap();
    let before = d.doc().clone();
    let ops = d
        .apply(
            sc(SongCommand::RemoveTrack {
                device: key("synth"),
            }),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, changed());
    assert_eq!(d.doc().song.tracks.len(), 1);
    assert_eq!(d.doc().song.tracks[0].device, key("lead"));
    assert_eq!(d.undo(&reg).unwrap(), changed());
    assert_eq!(d.doc(), &before);
    assert!(matches!(
        rejects(
            &mut d,
            sc(SongCommand::RemoveTrack {
                device: key("delay")
            }),
            &reg
        ),
        ModelError::BadSong(_)
    ));
}

#[test]
fn set_track_flags() {
    let reg = registry();
    let mut d = with_track(&reg);
    let before = d.doc().clone();
    let cmd = sc(SongCommand::SetTrackFlags {
        device: key("synth"),
        muted: true,
        solo: false,
        armed: true,
    });
    assert_eq!(d.apply(cmd, &reg).unwrap(), changed());
    let t = &d.doc().song.tracks[0];
    assert!(t.muted && !t.solo && t.armed);
    undo_all_restores(&mut d, &before, &reg);
    let bad = sc(SongCommand::SetTrackFlags {
        device: key("lead"),
        muted: true,
        solo: true,
        armed: true,
    });
    assert!(matches!(rejects(&mut d, bad, &reg), ModelError::BadSong(_)));
}

#[test]
fn track_flags_batch_as_one_undo_step() {
    let reg = registry();
    let mut d = with_track(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    let flags = |device: &str, armed: bool| {
        sc(SongCommand::SetTrackFlags {
            device: key(device),
            muted: false,
            solo: false,
            armed,
        })
    };
    d.apply(flags("synth", true), &reg).unwrap();
    let before = d.doc().clone();
    let batch = Command::Batch(vec![flags("synth", false), flags("lead", true)]);
    assert_eq!(d.apply(batch, &reg).unwrap(), changed());
    let t = &d.doc().song.tracks;
    assert!(!t[0].armed && t[1].armed);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "one undo reverts both tracks");
}

#[test]
fn add_automation_lane_creates_track_when_needed() {
    let reg = registry();
    let mut d = song_start(&reg);
    let before = d.doc().clone();
    let cmd = sc(SongCommand::AddAutomationLane {
        device: key("lead"),
        param: "filter.cutoff".into(),
    });
    assert_eq!(d.apply(cmd.clone(), &reg).unwrap(), changed());
    let t = d.doc().song.track(&key("lead")).unwrap();
    assert_eq!(t.lanes.len(), 2);
    assert_eq!(t.lanes[0].kind, LaneKind::Notes);
    assert_eq!(
        t.lanes[1].kind,
        LaneKind::Automation("filter.cutoff".into())
    );
    // One undo step removes the lane and the track created for it.
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    // Duplicate lane, unknown param, device without notes input, unknown device.
    assert!(matches!(rejects(&mut d, cmd, &reg), ModelError::BadSong(_)));
    let unknown = sc(SongCommand::AddAutomationLane {
        device: key("lead"),
        param: "filter.cutof".into(),
    });
    assert_eq!(
        rejects(&mut d, unknown, &reg),
        ModelError::UnknownKey("filter.cutof".into())
    );
    let delay = sc(SongCommand::AddAutomationLane {
        device: key("delay"),
        param: "mix".into(),
    });
    assert!(matches!(
        rejects(&mut d, delay, &reg),
        ModelError::BadSong(_)
    ));
    let nodev = sc(SongCommand::AddAutomationLane {
        device: key("zzz"),
        param: "mix".into(),
    });
    assert_eq!(
        rejects(&mut d, nodev, &reg),
        ModelError::UnknownDevice("zzz".into())
    );
}

// ---- clips ----------------------------------------------------------------------------------

fn with_lanes(reg: &Registry) -> Document {
    let mut d = with_track(reg);
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
        reg,
    )
    .unwrap();
    Document::new(d.doc().clone())
}

#[test]
fn add_clip_allocates_ids_and_keeps_lane_sorted() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    assert_eq!(d.last_clip_id(), None);
    let mut c = notes_clip(BAR, BAR, vec![note(0, 10, 60, 1.0)]);
    c.id = ClipId(99); // ignored
    assert_eq!(d.apply(add_clip("synth", 0, c), &reg).unwrap(), changed());
    assert_eq!(d.last_clip_id(), Some(ClipId(1)));
    d.apply(add_clip("synth", 0, notes_clip(0, BAR, vec![])), &reg)
        .unwrap();
    assert_eq!(d.last_clip_id(), Some(ClipId(2)));
    // Same start as clip 2: ordered by (start, id).
    d.apply(add_clip("synth", 0, notes_clip(0, 10, vec![])), &reg)
        .unwrap();
    let lane = &d.doc().song.tracks[0].lanes[0];
    let order: Vec<_> = lane.clips.iter().map(|c| c.id.0).collect();
    assert_eq!(order, [2, 3, 1]);
    assert_eq!(d.doc().song.next_clip_id, 4);
    let (t, l, c) = d.doc().song.clip(ClipId(1)).unwrap();
    assert_eq!((t.device.as_str(), l, c.start), ("synth", 0, BAR));
    assert!(d.doc().song.clip(ClipId(7)).is_none());
    // Undo frees nothing: ids are never reused while history may restore them.
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.next_clip_id, 3);
    d.apply(add_clip("synth", 0, notes_clip(0, 10, vec![])), &reg)
        .unwrap();
    assert_eq!(d.last_clip_id(), Some(ClipId(3)));
}

#[test]
fn add_clip_clamps_automation_values() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    let c = auto_clip(
        0,
        BAR,
        vec![
            point(0, 50_000.0, Curve::Linear),
            point(10, -5.0, Curve::Step),
        ],
    );
    d.apply(add_clip("synth", 1, c), &reg).unwrap();
    let ClipContent::Automation(p) = &d.doc().song.tracks[0].lanes[1].clips[0].content else {
        panic!()
    };
    assert_eq!((p[0].value, p[1].value), (20_000.0, 20.0));
}

#[test]
fn add_clip_rejections() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    let ok = || notes_clip(0, BAR, vec![note(0, 10, 60, 0.5), note(0, 10, 64, 0.5)]);
    let bad_song = |d: &mut Document, k: &str, lane: usize, c| {
        let e = rejects(d, add_clip(k, lane, c), &reg);
        assert!(
            matches!(e, ModelError::BadSong(_) | ModelError::OutOfRange(_)),
            "{e:?}"
        );
    };
    bad_song(&mut d, "lead", 0, ok()); // no track
    bad_song(&mut d, "synth", 2, ok()); // lane out of range
    bad_song(&mut d, "synth", 1, ok()); // notes on an automation lane
    bad_song(&mut d, "synth", 0, auto_clip(0, BAR, vec![])); // points on the note lane
    assert_eq!(
        rejects(&mut d, add_clip("zz", 0, ok()), &reg),
        ModelError::UnknownDevice("zz".into())
    );
    let geo = |f: &dyn Fn(&mut atomik_model::Clip)| {
        let mut c = ok();
        f(&mut c);
        c
    };
    for c in [
        geo(&|c| c.start = -1),
        geo(&|c| c.length = 0),
        geo(&|c| c.length = -5),
        geo(&|c| c.offset = -1),
        geo(&|c| c.loop_length = Some(0)),
        geo(&|c| c.start = atomik_model::song::MAX_TICK + 1),
    ] {
        bad_song(&mut d, "synth", 0, c);
    }
    for notes in [
        vec![note(0, 10, 64, 0.5), note(0, 10, 60, 0.5)], // unsorted keys
        vec![note(5, 10, 60, 0.5), note(0, 10, 60, 0.5)], // unsorted ticks
        vec![note(0, 10, 60, 0.5), note(0, 20, 60, 0.5)], // duplicate (tick, key)
        vec![note(-1, 10, 60, 0.5)],
        vec![note(0, 0, 60, 0.5)],
        vec![note(0, 10, 128, 0.5)],
        vec![note(0, 10, 60, 0.0)],
        vec![note(0, 10, 60, 1.01)],
        vec![note(0, 10, 60, f32::NAN)],
    ] {
        bad_song(&mut d, "synth", 0, notes_clip(0, BAR, notes));
    }
    for pts in [
        vec![point(5, 1000.0, Curve::Step), point(0, 1000.0, Curve::Step)],
        vec![point(5, 1000.0, Curve::Step), point(5, 900.0, Curve::Step)],
        vec![point(-1, 1000.0, Curve::Step)],
        vec![point(0, f32::NAN, Curve::Step)],
    ] {
        bad_song(&mut d, "synth", 1, auto_clip(0, BAR, pts));
    }
    assert_eq!(d.doc().song.next_clip_id, 1);
}

#[test]
fn remove_clip_and_undo() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    d.apply(add_clip("synth", 0, notes_clip(0, BAR, vec![])), &reg)
        .unwrap();
    let before = d.doc().clone();
    assert_eq!(
        d.apply(sc(SongCommand::RemoveClip { id: ClipId(1) }), &reg)
            .unwrap(),
        changed()
    );
    assert!(d.doc().song.tracks[0].lanes[0].clips.is_empty());
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    assert!(matches!(
        rejects(&mut d, sc(SongCommand::RemoveClip { id: ClipId(9) }), &reg),
        ModelError::UnknownKey(_)
    ));
}

fn geometry(id: u64, start: i64, length: i64, merge: Option<u64>) -> Command {
    sc(SongCommand::SetClipGeometry {
        id: ClipId(id),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        merge,
    })
}

#[test]
fn set_clip_geometry_moves_resorts_and_validates() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    d.apply(add_clip("synth", 0, notes_clip(0, BAR, vec![])), &reg)
        .unwrap();
    d.apply(add_clip("synth", 0, notes_clip(BAR, BAR, vec![])), &reg)
        .unwrap();
    let before = d.doc().clone();
    let cmd = sc(SongCommand::SetClipGeometry {
        id: ClipId(1),
        start: 2 * BAR,
        length: 3 * BAR,
        offset: 7,
        loop_length: Some(BAR),
        muted: true,
        merge: None,
    });
    assert_eq!(d.apply(cmd, &reg).unwrap(), changed());
    let lane = &d.doc().song.tracks[0].lanes[0];
    assert_eq!(lane.clips[0].id, ClipId(2));
    let c = &lane.clips[1];
    assert_eq!(
        (c.start, c.length, c.offset, c.loop_length, c.muted),
        (2 * BAR, 3 * BAR, 7, Some(BAR), true)
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    for cmd in [
        geometry(1, -1, BAR, None),
        geometry(1, 0, 0, None),
        geometry(9, 0, BAR, None),
    ] {
        rejects(&mut d, cmd, &reg);
    }
}

#[test]
fn set_clip_content_replaces_and_validates_kind() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    d.apply(add_clip("synth", 0, notes_clip(0, BAR, vec![])), &reg)
        .unwrap();
    d.apply(add_clip("synth", 1, auto_clip(0, BAR, vec![])), &reg)
        .unwrap();
    let mut d = Document::new(d.doc().clone());
    let before = d.doc().clone();
    let notes = ClipContent::Notes(vec![note(0, 5, 1, 0.25)]);
    let ops = d
        .apply(
            sc(SongCommand::SetClipContent {
                id: ClipId(1),
                content: notes.clone(),
                merge: None,
            }),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, changed());
    assert_eq!(d.doc().song.clip(ClipId(1)).unwrap().2.content, notes);
    d.apply(
        sc(SongCommand::SetClipContent {
            id: ClipId(2),
            content: ClipContent::Automation(vec![point(3, 1e9, Curve::Linear)]),
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(
        d.doc().song.clip(ClipId(2)).unwrap().2.content,
        ClipContent::Automation(vec![point(3, 20_000.0, Curve::Linear)])
    );
    undo_all_restores(&mut d, &before, &reg);
    for (id, content) in [
        (1, ClipContent::Automation(vec![])),
        (2, ClipContent::Notes(vec![])),
        (1, ClipContent::Notes(vec![note(0, 5, 200, 0.5)])),
        (3, ClipContent::Notes(vec![])),
    ] {
        rejects(
            &mut d,
            sc(SongCommand::SetClipContent {
                id: ClipId(id),
                content,
                merge: None,
            }),
            &reg,
        );
    }
}

#[test]
fn clip_drags_with_the_same_merge_id_merge_into_one_step() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    d.apply(add_clip("synth", 0, notes_clip(0, BAR, vec![])), &reg)
        .unwrap();
    d.apply(add_clip("synth", 0, notes_clip(0, BAR, vec![])), &reg)
        .unwrap();
    let start = d.doc().clone();
    for s in [10, 20, 30] {
        d.apply(geometry(1, s, BAR, Some(5)), &reg).unwrap();
    }
    // A content edit in the same drag group on the same clip joins the step.
    d.apply(
        sc(SongCommand::SetClipContent {
            id: ClipId(1),
            content: ClipContent::Notes(vec![note(0, 1, 1, 1.0)]),
            merge: Some(5),
        }),
        &reg,
    )
    .unwrap();
    let dragged = d.doc().clone();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &start, "the whole drag is one step");
    d.redo(&reg).unwrap();
    assert_eq!(d.doc(), &dragged);

    // end_merge splits and another id splits; another clip of the same group joins the step
    // (a drag of several clips is one step).
    d.apply(geometry(1, 40, BAR, Some(5)), &reg).unwrap();
    let a = d.doc().clone();
    d.end_merge();
    d.apply(geometry(1, 50, BAR, Some(5)), &reg).unwrap();
    d.apply(geometry(2, 50, BAR, Some(5)), &reg).unwrap();
    let b = d.doc().clone();
    d.apply(geometry(2, 60, BAR, Some(6)), &reg).unwrap();
    d.apply(geometry(2, 70, BAR, None), &reg).unwrap();
    d.apply(geometry(2, 80, BAR, None), &reg).unwrap();
    for _ in 0..3 {
        d.undo(&reg).unwrap();
    }
    assert_eq!(d.doc(), &b);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &a);
    // A param drag with the same id does not merge into a clip drag.
    d.apply(geometry(1, 90, BAR, Some(8)), &reg).unwrap();
    let c = d.doc().clone();
    d.apply(
        Command::SetParam {
            key: key("synth"),
            param: "osc.mix".into(),
            value: 0.1,
            merge: Some(8),
        },
        &reg,
    )
    .unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &c);
}

#[test]
fn split_clip_keeps_content_alignment() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    let mut c = notes_clip(
        BAR,
        2 * BAR,
        vec![
            note(0, 10, 60, 0.5),
            note(PPQ, 10, 62, 0.5),
            note(3 * PPQ, 10, 64, 0.5),
        ],
    );
    c.offset = 5;
    d.apply(add_clip("synth", 0, c), &reg).unwrap();
    let track_before = d.doc().song.tracks[0].clone();
    let before = d.doc().clone();
    let ops = d
        .apply(
            sc(SongCommand::SplitClip {
                id: ClipId(1),
                at: BAR + 2 * PPQ,
            }),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, changed());
    assert_eq!(d.last_clip_id(), Some(ClipId(2)));
    let lane = &d.doc().song.tracks[0].lanes[0];
    let (l, r) = (&lane.clips[0], &lane.clips[1]);
    assert_eq!(
        (l.id, l.start, l.length, l.offset),
        (ClipId(1), BAR, 2 * PPQ, 5)
    );
    assert_eq!(
        (r.id, r.start, r.length, r.offset),
        (ClipId(2), BAR + 2 * PPQ, 2 * BAR - 2 * PPQ, 5 + 2 * PPQ)
    );
    assert_eq!(l.content, r.content);
    // No note crosses the split, so playback is unchanged.
    assert_eq!(
        atomik_model::song::flatten_notes(&d.doc().song.tracks[0]),
        atomik_model::song::flatten_notes(&track_before)
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    for at in [BAR, 3 * BAR, 0, 10 * BAR] {
        rejects(
            &mut d,
            sc(SongCommand::SplitClip { id: ClipId(1), at }),
            &reg,
        );
    }
    rejects(
        &mut d,
        sc(SongCommand::SplitClip {
            id: ClipId(5),
            at: BAR + 1,
        }),
        &reg,
    );
}

#[test]
fn split_looped_clip_normalizes_offset_into_the_loop() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    let mut c = notes_clip(
        0,
        4 * BAR,
        vec![note(0, 10, 60, 0.5), note(PPQ, 10, 62, 0.5)],
    );
    c.loop_length = Some(BAR);
    d.apply(add_clip("synth", 0, c), &reg).unwrap();
    let flat = atomik_model::song::flatten_notes(&d.doc().song.tracks[0]);
    d.apply(
        sc(SongCommand::SplitClip {
            id: ClipId(1),
            at: 2 * BAR + PPQ,
        }),
        &reg,
    )
    .unwrap();
    let r = &d.doc().song.tracks[0].lanes[0].clips[1];
    assert_eq!((r.offset, r.loop_length), (PPQ, Some(BAR)));
    assert_eq!(
        atomik_model::song::flatten_notes(&d.doc().song.tracks[0]),
        flat
    );
}

#[test]
fn duplicate_clip_copies_to_a_new_start() {
    let reg = registry();
    let mut d = with_lanes(&reg);
    let mut c = auto_clip(0, BAR, vec![point(0, 500.0, Curve::Linear)]);
    c.loop_length = Some(PPQ);
    c.muted = true;
    d.apply(add_clip("synth", 1, c), &reg).unwrap();
    let before = d.doc().clone();
    assert_eq!(
        d.apply(
            sc(SongCommand::DuplicateClip {
                id: ClipId(1),
                start: 4 * BAR
            }),
            &reg
        )
        .unwrap(),
        changed()
    );
    assert_eq!(d.last_clip_id(), Some(ClipId(2)));
    let lane = &d.doc().song.tracks[0].lanes[1];
    let mut expected = lane.clips[0].clone();
    expected.id = ClipId(2);
    expected.start = 4 * BAR;
    assert_eq!(lane.clips[1], expected);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    rejects(
        &mut d,
        sc(SongCommand::DuplicateClip {
            id: ClipId(1),
            start: -1,
        }),
        &reg,
    );
    rejects(
        &mut d,
        sc(SongCommand::DuplicateClip {
            id: ClipId(3),
            start: 0,
        }),
        &reg,
    );
}

// ---- tempo, meter, loop, end ----------------------------------------------------------------

#[test]
fn set_tempo_keeps_the_song_tempo_in_sync() {
    let reg = registry();
    let mut d = Document::new(atomik_model::empty_doc());
    assert_tempo_invariant(d.doc());
    d.apply(Command::SetTempo(97.25), &reg).unwrap();
    assert_eq!(d.doc().song.tempo[0].bpm, 97.25);
    assert_tempo_invariant(d.doc());
    d.undo(&reg).unwrap();
    assert_tempo_invariant(d.doc());
    assert_eq!(d.doc().song, empty_song(120.0));
}

#[test]
fn set_tempo_map_updates_rack_tempo() {
    let reg = registry();
    let mut d = with_track(&reg);
    let before = d.doc().clone();
    let map = vec![TempoEvent::step(0, 90.5), TempoEvent::step(BAR, 140.0)];
    let ops = d
        .apply(sc(SongCommand::SetTempoMap(map.clone())), &reg)
        .unwrap();
    assert_eq!(ops, [SyncOp::TempoChanged(90.5), SyncOp::SongChanged]);
    assert_eq!(d.doc().song.tempo, map);
    assert_eq!(d.doc().tempo_bpm, 90.5);
    assert_tempo_invariant(d.doc());
    let ops = d.undo(&reg).unwrap();
    assert_eq!(ops, [SyncOp::TempoChanged(120.0), SyncOp::SongChanged]);
    assert_eq!(d.doc(), &before);
    let ev = |tick, bpm| TempoEvent::step(tick, bpm);
    for bad in [
        vec![],
        vec![ev(5, 120.0)],
        vec![ev(0, 120.0), ev(0, 130.0)],
        vec![ev(0, 120.0), ev(BAR, 130.0), ev(10, 100.0)],
        vec![ev(0, 19.9)],
        vec![ev(0, 120.0), ev(BAR, 1000.0)],
        vec![ev(0, f64::NAN)],
    ] {
        rejects(&mut d, sc(SongCommand::SetTempoMap(bad)), &reg);
    }
}

#[test]
fn set_meter_loop_and_end() {
    let reg = registry();
    let mut d = with_track(&reg);
    let before = d.doc().clone();
    let meter = |b, u| {
        sc(SongCommand::SetMeter {
            beats_per_bar: b,
            beat_unit: u,
        })
    };
    assert_eq!(d.apply(meter(7, 8), &reg).unwrap(), changed());
    assert_eq!(d.doc().song.meter(), (7, 8));
    for (b, u) in [(0, 4), (33, 4), (4, 3), (4, 0), (4, 32)] {
        rejects(&mut d, meter(b, u), &reg);
    }
    let lp = |s, e, en| {
        sc(SongCommand::SetLoop {
            range: (s, e),
            enabled: en,
        })
    };
    assert_eq!(d.apply(lp(BAR, 2 * BAR, true), &reg).unwrap(), changed());
    assert_eq!(d.doc().song.loop_range, (BAR, 2 * BAR));
    assert!(d.doc().song.loop_enabled);
    for (s, e) in [(BAR, BAR), (2 * BAR, BAR), (-1, BAR)] {
        rejects(&mut d, lp(s, e, true), &reg);
    }
    assert_eq!(
        d.apply(sc(SongCommand::SetSongEnd(32 * BAR)), &reg)
            .unwrap(),
        changed()
    );
    assert_eq!(d.doc().song.end, 32 * BAR);
    for e in [0, -5] {
        rejects(&mut d, sc(SongCommand::SetSongEnd(e)), &reg);
    }
    undo_all_restores(&mut d, &before, &reg);
}

// ---- rack interaction -----------------------------------------------------------------------

#[test]
fn remove_device_removes_its_track_and_undo_restores_both() {
    let reg = registry();
    let mut d = with_track(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(
        add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, 100, 60, 0.5)])),
        &reg,
    )
    .unwrap();
    let before = d.doc().clone();
    let ops = d
        .apply(Command::RemoveDevice { key: key("synth") }, &reg)
        .unwrap();
    // The song stops referencing the device before the engine removes it.
    assert_eq!(
        ops,
        [
            SyncOp::SongChanged,
            routing(),
            SyncOp::DeviceRemoved(key("synth"))
        ]
    );
    assert!(d.doc().song.track(&key("synth")).is_none());
    assert_eq!(d.doc().song.tracks.len(), 1);
    let ops = d.undo(&reg).unwrap();
    assert_eq!(ops.first(), Some(&SyncOp::DeviceAdded(key("synth"))));
    assert_eq!(ops.last(), Some(&SyncOp::SongChanged));
    assert_eq!(d.doc(), &before, "device and track (index, clips) restored");
    let ops = d.redo(&reg).unwrap();
    assert_eq!(ops[0], SyncOp::SongChanged);
    assert!(d.doc().song.track(&key("synth")).is_none());
}

// ---- tempo ramps and the meter map ----------------------------------------------------

fn ramp(tick: i64, bpm: f64) -> TempoEvent {
    TempoEvent {
        tick,
        bpm,
        ramp: true,
    }
}

fn mev(bar: u32, num: u8, den: u8) -> MeterEvent {
    MeterEvent { bar, num, den }
}

#[test]
fn a_tempo_ramp_is_stored_and_undone_like_a_step() {
    let reg = registry();
    let mut d = with_track(&reg);
    let before = d.doc().clone();
    let map = vec![ramp(0, 60.0), TempoEvent::step(2 * BAR, 180.0)];
    let ops = d
        .apply(sc(SongCommand::SetTempoMap(map.clone())), &reg)
        .unwrap();
    assert_eq!(ops, [SyncOp::TempoChanged(60.0), SyncOp::SongChanged]);
    assert_eq!(d.doc().song.tempo, map);
    assert_tempo_invariant(d.doc());
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().song.tempo, map);
}

#[test]
fn a_ramp_on_the_last_event_is_rejected() {
    let reg = registry();
    let mut d = with_track(&reg);
    for bad in [
        vec![ramp(0, 100.0)],
        vec![TempoEvent::step(0, 100.0), ramp(BAR, 120.0)],
    ] {
        let e = rejects(&mut d, sc(SongCommand::SetTempoMap(bad)), &reg);
        assert!(e.to_string().contains("cannot ramp"), "{e}");
    }
}

#[test]
fn set_tempo_keeps_the_first_events_ramp() {
    let reg = registry();
    let mut d = with_track(&reg);
    let map = vec![ramp(0, 100.0), TempoEvent::step(BAR, 160.0)];
    d.apply(sc(SongCommand::SetTempoMap(map)), &reg).unwrap();
    d.apply(Command::SetTempo(110.0), &reg).unwrap();
    assert_eq!(d.doc().song.tempo[0], ramp(0, 110.0));
    assert_tempo_invariant(d.doc());
}

#[test]
fn dragging_the_tempo_map_is_one_undo_step() {
    let reg = registry();
    let mut d = with_track(&reg);
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 120.0),
            TempoEvent::step(BAR, 140.0),
        ])),
        &reg,
    )
    .unwrap();
    let before = d.doc().clone();
    for bpm in [141.0, 150.0, 160.0] {
        d.apply(
            sc(SongCommand::DragTempoMap {
                map: vec![TempoEvent::step(0, 120.0), TempoEvent::step(BAR, bpm)],
                merge: 7,
            }),
            &reg,
        )
        .unwrap();
    }
    assert_eq!(d.doc().song.tempo[1].bpm, 160.0);
    d.apply(
        sc(SongCommand::DragTempoMap {
            map: vec![TempoEvent::step(0, 120.0), TempoEvent::step(BAR, 170.0)],
            merge: 8,
        }),
        &reg,
    )
    .unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(
        d.doc().song.tempo[1].bpm,
        160.0,
        "another drag is another step"
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    // A rejected drag step changes nothing.
    rejects(
        &mut d,
        sc(SongCommand::DragTempoMap {
            map: vec![TempoEvent::step(5, 120.0)],
            merge: 9,
        }),
        &reg,
    );
}

#[test]
fn the_meter_map_is_set_validated_and_undone() {
    let reg = registry();
    let mut d = with_track(&reg);
    let before = d.doc().clone();
    let map = vec![mev(1, 4, 4), mev(3, 3, 4), mev(7, 6, 8)];
    assert_eq!(
        d.apply(sc(SongCommand::SetMeterMap(map.clone())), &reg)
            .unwrap(),
        changed()
    );
    assert_eq!(d.doc().song.meter, map);
    assert_eq!(d.doc().song.meter(), (4, 4));
    assert_eq!(d.doc().song.bar_ticks_at(2 * BAR), 3 * PPQ);
    for bad in [
        vec![],
        vec![mev(2, 4, 4)],
        vec![mev(1, 4, 4), mev(1, 3, 4)],
        vec![mev(1, 4, 4), mev(5, 3, 4), mev(4, 2, 4)],
        vec![mev(1, 0, 4)],
        vec![mev(1, 4, 3)],
        vec![mev(1, 33, 4)],
        vec![mev(1, 4, 4), mev(u32::MAX, 4, 4)],
    ] {
        rejects(&mut d, sc(SongCommand::SetMeterMap(bad)), &reg);
    }
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn set_meter_changes_bar_one_and_keeps_later_changes() {
    let reg = registry();
    let mut d = with_track(&reg);
    d.apply(
        sc(SongCommand::SetMeterMap(vec![mev(1, 4, 4), mev(5, 3, 4)])),
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetMeter {
            beats_per_bar: 7,
            beat_unit: 8,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().song.meter, vec![mev(1, 7, 8), mev(5, 3, 4)]);
}

#[test]
fn dragging_a_meter_change_is_one_undo_step() {
    let reg = registry();
    let mut d = with_track(&reg);
    d.apply(
        sc(SongCommand::SetMeterMap(vec![mev(1, 4, 4), mev(3, 3, 4)])),
        &reg,
    )
    .unwrap();
    let before = d.doc().clone();
    for bar in [4, 5, 6] {
        d.apply(
            sc(SongCommand::DragMeterMap {
                map: vec![mev(1, 4, 4), mev(bar, 3, 4)],
                merge: 1,
            }),
            &reg,
        )
        .unwrap();
    }
    assert_eq!(d.doc().song.meter[1].bar, 6);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn frame_at_follows_a_tempo_ramp() {
    let reg = registry();
    let mut d = with_track(&reg);
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            ramp(0, 60.0),
            TempoEvent::step(2 * BAR, 180.0),
        ])),
        &reg,
    )
    .unwrap();
    let song = &d.doc().song;
    // The ramp takes 2 * 4 / avg seconds; the closed form is 60 / PPQ * L / (b1 - b0) * ln(b1 / b0).
    let secs = 60.0 / PPQ as f64 * (2 * BAR) as f64 / 120.0 * (180.0f64 / 60.0).ln();
    let frames = song.frame_at(2 * BAR, 48_000);
    assert!((frames as f64 - secs * 48_000.0).abs() <= 0.5, "{frames}");
    // After the ramp the tempo holds at 180.
    let one_beat = song.frame_at(2 * BAR + PPQ, 48_000) - frames;
    assert_eq!(one_beat, 16_000);
}

#[test]
fn nearest_bar_line_follows_the_meter_map() {
    let reg = registry();
    let mut d = with_track(&reg);
    d.apply(
        sc(SongCommand::SetMeterMap(vec![mev(1, 4, 4), mev(3, 3, 4)])),
        &reg,
    )
    .unwrap();
    let song = &d.doc().song;
    // Bars: 0, 4 PPQ, 8 PPQ, 11 PPQ, 14 PPQ...
    assert_eq!(song.nearest_bar_line(-5), 0);
    assert_eq!(song.nearest_bar_line(2 * PPQ - 1), 0);
    assert_eq!(song.nearest_bar_line(2 * PPQ), BAR);
    assert_eq!(song.nearest_bar_line(2 * BAR + PPQ), 2 * BAR);
    assert_eq!(song.nearest_bar_line(2 * BAR + 2 * PPQ), 2 * BAR + 3 * PPQ);
    assert_eq!(
        song.nearest_bar_line(2 * BAR + 3 * PPQ / 2),
        2 * BAR + 3 * PPQ,
        "halves round up"
    );
}
