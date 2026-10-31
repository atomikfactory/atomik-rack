//! Workflow clip, note and point commands (model half):
//! snapshots, paste at a position and track, duplicate, join, the neighbour helpers, and
//! rejections that change nothing.

mod common;

use atomik_model::{
    Clip, ClipContent, ClipId, ClipSnapshot, Command, Curve, Document, LaneKind, ModelError,
    NoteSnapshot, PointSnapshot, SongCommand, SyncOp, WorkflowCommand, empty_doc, neighbour_clip,
    neighbour_device, neighbour_track,
};
use common::audio::*;
use common::song::*;
use common::workflow::*;
use common::*;

/// Two tracks (synth, pad), the synth with two note clips and an automation lane.
fn arranged(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("synth")),
            add(1, SYNTH, Some("pad")),
            add_track("synth"),
            add_track("pad"),
            add_clip(
                "synth",
                0,
                notes_clip(
                    BAR,
                    BAR,
                    vec![note(0, PPQ, 60, 0.8), note(PPQ, PPQ, 64, 0.6)],
                ),
            ),
            add_clip(
                "synth",
                0,
                notes_clip(3 * BAR, 2 * BAR, vec![note(0, PPQ, 67, 0.5)]),
            ),
            add_clip(
                "pad",
                0,
                notes_clip(2 * BAR, BAR, vec![note(0, 2 * PPQ, 48, 1.0)]),
            ),
        ],
    );
    d
}

fn ids(d: &Document) -> Vec<ClipId> {
    clip_ids(d.doc())
}

fn clip_of(d: &Document, id: ClipId) -> Clip {
    d.doc().song.clip(id).unwrap().2.clone()
}

fn paste_clips(track: Option<&str>, at_tick: i64, s: &ClipSnapshot) -> Command {
    wf(WorkflowCommand::PasteClips {
        track: track.map(key),
        lane: None,
        at_tick,
        clips: s.clone(),
    })
}

// ---------------------------------------------------------------- clip snapshots

#[test]
fn a_clip_snapshot_keeps_track_relative_offsets_the_origin_and_the_span() {
    let r = registry();
    let d = arranged(&r);
    let all = ids(&d);
    let s = ClipSnapshot::capture(d.doc(), &all).unwrap();
    // synth BAR..2BAR, pad 2BAR..3BAR, synth 3BAR..5BAR, in time order.
    assert_eq!(s.origin, BAR);
    assert_eq!(s.span, 4 * BAR);
    let rel: Vec<i64> = s.entries.iter().map(|e| e.clip.start).collect();
    assert_eq!(rel, [0, BAR, 2 * BAR]);
    let on: Vec<&str> = s.entries.iter().map(|e| e.device.as_str()).collect();
    assert_eq!(on, ["synth", "pad", "synth"]);
    assert!(s.entries.iter().all(|e| e.lane == LaneKind::Notes));
}

#[test]
fn a_clip_snapshot_rejects_empty_and_unknown_selections() {
    let r = registry();
    let d = arranged(&r);
    assert!(matches!(
        ClipSnapshot::capture(d.doc(), &[]),
        Err(ModelError::BadSong(_))
    ));
    assert!(matches!(
        ClipSnapshot::capture(d.doc(), &[ClipId(999)]),
        Err(ModelError::UnknownKey(_))
    ));
}

// ---------------------------------------------------------------- paste clips

#[test]
fn pasting_clips_keeps_their_layout_on_their_source_tracks() {
    let r = registry();
    let mut d = arranged(&r);
    let all = ids(&d);
    let s = ClipSnapshot::capture(d.doc(), &all).unwrap();
    let next = d.doc().song.next_clip_id;
    let ops = one_step(&mut d, &r, paste_clips(None, 10 * BAR, &s));
    assert_eq!(ops, [SyncOp::SongChanged]);
    let synth = &d.doc().song.track(&key("synth")).unwrap().lanes[0].clips;
    let starts: Vec<i64> = synth.iter().map(|c| c.start).collect();
    assert_eq!(starts, [BAR, 3 * BAR, 10 * BAR, 12 * BAR]);
    let pad = &d.doc().song.track(&key("pad")).unwrap().lanes[0].clips;
    let starts: Vec<i64> = pad.iter().map(|c| c.start).collect();
    assert_eq!(starts, [2 * BAR, 11 * BAR]);
    // Ids come from the next free id upward, in snapshot order; the last one is reported.
    assert_eq!(d.doc().song.next_clip_id, next + 3);
    assert_eq!(d.last_clip_id(), Some(ClipId(next + 2)));
    assert_eq!(clip_of(&d, ClipId(next)).start, 10 * BAR);
    assert_eq!(clip_of(&d, ClipId(next + 1)).start, 11 * BAR);
    // Content and geometry come with the clip.
    assert_eq!(clip_of(&d, ClipId(next + 2)).length, 2 * BAR);
}

#[test]
fn repeated_pastes_laid_end_to_end_advance_by_the_span() {
    let r = registry();
    let mut d = arranged(&r);
    let first = ids(&d)[0];
    let s = ClipSnapshot::capture(d.doc(), &[first]).unwrap();
    let mut at = 8 * BAR;
    for _ in 0..3 {
        d.apply(paste_clips(None, at, &s), &r).unwrap();
        at += s.span;
    }
    let synth = &d.doc().song.track(&key("synth")).unwrap().lanes[0].clips;
    let starts: Vec<i64> = synth.iter().map(|c| c.start).collect();
    assert_eq!(starts, [BAR, 3 * BAR, 8 * BAR, 9 * BAR, 10 * BAR]);
}

#[test]
fn a_selected_track_receives_every_pasted_clip() {
    let r = registry();
    let mut d = arranged(&r);
    let all = ids(&d);
    let s = ClipSnapshot::capture(d.doc(), &all).unwrap();
    one_step(&mut d, &r, paste_clips(Some("pad"), 20 * BAR, &s));
    let pad = &d.doc().song.track(&key("pad")).unwrap().lanes[0].clips;
    assert_eq!(pad.len(), 1 + 3);
    assert_eq!(
        d.doc().song.track(&key("synth")).unwrap().lanes[0]
            .clips
            .len(),
        2
    );
}

#[test]
fn automation_clips_need_the_same_lane_on_the_target_track() {
    let r = registry();
    let mut d = arranged(&r);
    run(
        &mut d,
        &r,
        [
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
            add_clip(
                "synth",
                1,
                auto_clip(0, BAR, vec![point(0, 500.0, Curve::Linear)]),
            ),
        ],
    );
    let auto = d.last_clip_id().unwrap();
    let s = ClipSnapshot::capture(d.doc(), &[auto]).unwrap();
    assert_eq!(
        s.entries[0].lane,
        LaneKind::Automation("filter.cutoff".into())
    );
    // The source track has the lane: the paste lands on it.
    one_step(&mut d, &r, paste_clips(None, 4 * BAR, &s));
    assert_eq!(
        d.doc().song.track(&key("synth")).unwrap().lanes[1]
            .clips
            .len(),
        2
    );
    // The pad has no such lane: rejected, nothing changes.
    let e = rejected(&mut d, &r, paste_clips(Some("pad"), 4 * BAR, &s));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn pasting_on_a_missing_track_or_before_zero_is_rejected() {
    let r = registry();
    let mut d = arranged(&r);
    let s = ClipSnapshot::capture(d.doc(), &ids(&d)).unwrap();
    let e = rejected(&mut d, &r, paste_clips(Some("ghost"), 0, &s));
    assert!(matches!(
        e,
        ModelError::BadSong(_) | ModelError::UnknownDevice(_)
    ));
    let e = rejected(&mut d, &r, paste_clips(None, -BAR, &s));
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    // Cross-song: the source tracks do not exist in an empty song.
    let mut other = Document::new(empty_doc());
    let e = rejected(&mut other, &r, paste_clips(None, 0, &s));
    assert!(matches!(
        e,
        ModelError::BadSong(_) | ModelError::UnknownDevice(_)
    ));
}

#[test]
fn note_clips_do_not_paste_onto_an_audio_track() {
    let r = audio_registry();
    let mut d = with_audio_track(&r);
    run(
        &mut d,
        &r,
        [
            add(1, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
        ],
    );
    let s = ClipSnapshot::capture(d.doc(), &ids(&d)).unwrap();
    let e = rejected(&mut d, &r, paste_clips(Some("audio-track"), BAR, &s));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn audio_clips_paste_across_songs_once_the_pool_has_the_asset() {
    let r = audio_registry();
    let mut a = with_audio_track(&r);
    a.apply(add_audio_clip("audio-track", BAR, BAR, 1), &r)
        .unwrap();
    let s = ClipSnapshot::capture(a.doc(), &ids(&a)).unwrap();
    assert!(s.assets.contains_key(&hash(1)));
    assert!(s.required_assets().contains(&hash(1)));
    // Another song with the same audio track but no sample pool.
    let mut b = Document::new(empty_doc());
    b.apply(add_audio_track(), &r).unwrap();
    let e = rejected(&mut b, &r, paste_clips(Some("audio-track"), 0, &s));
    assert!(
        matches!(&e, ModelError::UnknownKey(m) if m.contains("sample pool")),
        "the paste names the missing pool entry: {e:?}"
    );
    b.apply(
        Command::RegisterAsset {
            hash: hash(1),
            meta: s.assets[&hash(1)].clone(),
        },
        &r,
    )
    .unwrap();
    one_step(&mut b, &r, paste_clips(Some("audio-track"), 2 * BAR, &s));
    let clips = &b.doc().song.track(&key("audio-track")).unwrap().lanes[0].clips;
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].start, 2 * BAR);
}

// ---------------------------------------------------------------- duplicate clips

#[test]
fn duplicating_clips_copies_the_selection_after_itself_keeping_its_layout() {
    let r = registry();
    let mut d = arranged(&r);
    let all = ids(&d);
    // The first synth clip and the pad clip: BAR..3BAR.
    let (a, b) = (all[0], all[2]);
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateClips { ids: vec![b, a] }),
    );
    assert_eq!(ops, [SyncOp::SongChanged]);
    let synth = &d.doc().song.track(&key("synth")).unwrap().lanes[0].clips;
    // The selection spans BAR..3BAR (two bars): the copies start at 3BAR and 4BAR.
    let starts: Vec<i64> = synth.iter().map(|c| c.start).collect();
    assert_eq!(starts, [BAR, 3 * BAR, 3 * BAR,]);
    let pad = &d.doc().song.track(&key("pad")).unwrap().lanes[0].clips;
    let starts: Vec<i64> = pad.iter().map(|c| c.start).collect();
    assert_eq!(starts, [2 * BAR, 4 * BAR]);
}

#[test]
fn duplicating_one_clip_matches_the_single_clip_command() {
    let r = registry();
    let mut a = arranged(&r);
    let mut b = arranged(&r);
    let id = ids(&a)[0];
    a.apply(wf(WorkflowCommand::DuplicateClips { ids: vec![id] }), &r)
        .unwrap();
    b.apply(
        sc(SongCommand::DuplicateClip {
            id,
            start: BAR + BAR,
        }),
        &r,
    )
    .unwrap();
    assert_eq!(text(&a), text(&b));
}

#[test]
fn duplicating_unknown_or_no_clips_is_rejected() {
    let r = registry();
    let mut d = arranged(&r);
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateClips { ids: Vec::new() }),
    );
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateClips {
            ids: vec![ClipId(77)],
        }),
    );
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
}

// ---------------------------------------------------------------- join

#[test]
fn joining_note_clips_makes_one_clip_over_both_with_every_note_in_place() {
    let r = registry();
    let mut d = arranged(&r);
    let all = ids(&d);
    let (first, second) = (all[0], all[1]); // synth: BAR..2BAR and 3BAR..5BAR
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::JoinClips {
            ids: vec![second, first],
        }),
    );
    assert_eq!(ops, [SyncOp::SongChanged]);
    let clips = &d.doc().song.track(&key("synth")).unwrap().lanes[0].clips;
    assert_eq!(clips.len(), 1);
    let c = &clips[0];
    assert_eq!(
        (c.id, c.start, c.length, c.offset),
        (first, BAR, 4 * BAR, 0)
    );
    let ClipContent::Notes(n) = &c.content else {
        panic!("notes")
    };
    let got: Vec<(i64, u8)> = n.iter().map(|x| (x.tick, x.key)).collect();
    assert_eq!(got, [(0, 60), (PPQ, 64), (2 * BAR, 67)]);
}

#[test]
fn joining_cuts_the_content_to_what_each_clip_shows() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    let mut shifted = notes_clip(
        BAR,
        BAR,
        vec![
            note(0, PPQ, 50, 0.5),
            note(PPQ, 4 * BAR, 52, 0.5),
            note(2 * BAR, PPQ, 54, 0.5),
        ],
    );
    shifted.offset = PPQ;
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip("synth", 0, shifted),
            add_clip(
                "synth",
                0,
                notes_clip(2 * BAR, BAR, vec![note(0, PPQ, 70, 0.9)]),
            ),
        ],
    );
    let all = ids(&d);
    d.apply(wf(WorkflowCommand::JoinClips { ids: all }), &r)
        .unwrap();
    let c = &d.doc().song.track(&key("synth")).unwrap().lanes[0].clips[0];
    let ClipContent::Notes(n) = &c.content else {
        panic!("notes")
    };
    // The first clip shows content PPQ.. PPQ+BAR: the note at 0 is before the window, the long
    // note at PPQ starts at the clip start and is cut to the window, the one at 2BAR is after.
    let got: Vec<(i64, i64, u8)> = n.iter().map(|x| (x.tick, x.length, x.key)).collect();
    assert_eq!(got, [(0, BAR, 52), (BAR, PPQ, 70)]);
}

#[test]
fn joining_leaves_a_muted_clip_silent_instead_of_folding_its_notes_into_an_audible_one() {
    use atomik_model::song::flatten_notes;
    let r = registry();
    let mut d = arranged(&r);
    let all = ids(&d); // synth: BAR..2BAR (key 60, 64) and 3BAR..5BAR (key 67), then pad
    let (first, last) = (all[0], all[1]);
    // A third synth clip, muted, between the two audible ones.
    run(
        &mut d,
        &r,
        [add_clip(
            "synth",
            0,
            notes_clip(2 * BAR, BAR, vec![note(0, PPQ, 90, 0.9)]),
        )],
    );
    let lane_ids: Vec<ClipId> = d.doc().song.track(&key("synth")).unwrap().lanes[0]
        .clips
        .iter()
        .map(|c| c.id)
        .collect();
    let third = lane_ids[1];
    assert!(third != first && third != last);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::SetClipsMuted {
            ids: vec![third],
            muted: true,
        }),
    );
    let before = flatten_notes(d.doc().song.track(&key("synth")).unwrap());
    assert!(before.iter().all(|n| n.2 != 90));
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::JoinClips {
            ids: vec![first, third, last],
        }),
    );
    let t = d.doc().song.track(&key("synth")).unwrap();
    let keys: Vec<u8> = flatten_notes(t).iter().map(|n| n.2).collect();
    assert!(!keys.contains(&90), "the muted clip's note became audible");
    assert!(keys.contains(&60) && keys.contains(&67));
    // The muted clip is still there, still muted.
    let c = t.lanes[0].clips.iter().find(|c| c.id == third).unwrap();
    assert!(c.muted);
    // Muted clips alone still join, into a muted clip.
    let mut d = arranged(&r);
    let all = ids(&d);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::SetClipsMuted {
            ids: vec![all[0], all[1]],
            muted: true,
        }),
    );
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::JoinClips {
            ids: vec![all[0], all[1]],
        }),
    );
    let clips = &d.doc().song.track(&key("synth")).unwrap().lanes[0].clips;
    assert_eq!(clips.len(), 1);
    assert!(clips[0].muted);
    // One audible clip with a muted one is not enough to join.
    let mut d = arranged(&r);
    let all = ids(&d);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::SetClipsMuted {
            ids: vec![all[0]],
            muted: true,
        }),
    );
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::JoinClips {
            ids: vec![all[0], all[1]],
        }),
    );
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn joining_unlooped_automation_clips_merges_points_later_clip_wins() {
    let r = registry();
    let mut d = arranged(&r);
    run(
        &mut d,
        &r,
        [
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
            add_clip(
                "synth",
                1,
                auto_clip(
                    0,
                    BAR,
                    vec![
                        point(0, 500.0, Curve::Linear),
                        point(PPQ, 600.0, Curve::Step),
                    ],
                ),
            ),
            add_clip(
                "synth",
                1,
                auto_clip(BAR - PPQ, BAR, vec![point(PPQ, 900.0, Curve::Linear)]),
            ),
        ],
    );
    let auto: Vec<ClipId> = d.doc().song.track(&key("synth")).unwrap().lanes[1]
        .clips
        .iter()
        .map(|c| c.id)
        .collect();
    one_step(&mut d, &r, wf(WorkflowCommand::JoinClips { ids: auto }));
    let c = &d.doc().song.track(&key("synth")).unwrap().lanes[1].clips[0];
    assert_eq!((c.start, c.length), (0, 2 * BAR - PPQ));
    let ClipContent::Automation(p) = &c.content else {
        panic!("automation")
    };
    let got: Vec<i64> = p.iter().map(|x| x.tick).collect();
    assert_eq!(got, [0, PPQ, BAR]);
}

#[test]
fn join_needs_two_unlooped_clips_of_one_lane_and_no_audio() {
    let r = audio_registry();
    let mut d = arranged_audio(&r);
    let notes: Vec<ClipId> = d.doc().song.track(&key("synth")).unwrap().lanes[0]
        .clips
        .iter()
        .map(|c| c.id)
        .collect();
    let join = |ids: Vec<ClipId>| wf(WorkflowCommand::JoinClips { ids });
    // One clip only.
    let e = rejected(&mut d, &r, join(vec![notes[0]]));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    // Two tracks.
    let other = d.doc().song.track(&key("pad")).unwrap().lanes[0].clips[0].id;
    let e = rejected(&mut d, &r, join(vec![notes[0], other]));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    // A looped clip.
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: notes[0],
            start: BAR,
            length: BAR,
            offset: 0,
            loop_length: Some(PPQ),
            muted: false,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    let e = rejected(&mut d, &r, join(vec![notes[0], notes[1]]));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    // Audio clips.
    let audio: Vec<ClipId> = d.doc().song.track(&key("audio-track")).unwrap().lanes[0]
        .clips
        .iter()
        .map(|c| c.id)
        .collect();
    let e = rejected(&mut d, &r, join(audio));
    assert!(
        matches!(&e, ModelError::BadSong(m) if m.contains("audio")),
        "{e:?}"
    );
    let e = rejected(&mut d, &r, join(vec![ClipId(404), notes[1]]));
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
}

fn arranged_audio(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = with_audio_track(r);
    run(
        &mut d,
        r,
        [
            add(1, SYNTH, Some("synth")),
            add(2, SYNTH, Some("pad")),
            add_track("synth"),
            add_track("pad"),
            add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
            add_clip(
                "synth",
                0,
                notes_clip(2 * BAR, BAR, vec![note(0, PPQ, 62, 0.8)]),
            ),
            add_clip("pad", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
            add_audio_clip("audio-track", 0, BAR, 1),
            add_audio_clip("audio-track", 2 * BAR, BAR, 1),
        ],
    );
    d
}

// ---------------------------------------------------------------- notes

fn clip_notes(d: &Document, id: ClipId) -> Vec<(i64, i64, u8)> {
    match &clip_of(d, id).content {
        ClipContent::Notes(n) => n.iter().map(|x| (x.tick, x.length, x.key)).collect(),
        _ => panic!("notes"),
    }
}

#[test]
fn a_note_snapshot_is_relative_to_the_earliest_note() {
    let r = registry();
    let d = arranged(&r);
    let clip = clip_of(&d, ids(&d)[0]);
    let s = NoteSnapshot::capture(&clip, &[1]).unwrap();
    assert_eq!(s.notes.len(), 1);
    assert_eq!((s.notes[0].tick, s.span), (0, PPQ));
    let s = NoteSnapshot::capture(&clip, &[1, 0, 1]).unwrap();
    let ticks: Vec<i64> = s.notes.iter().map(|n| n.tick).collect();
    assert_eq!(ticks, [0, PPQ]);
    assert_eq!(s.span, 2 * PPQ);
    assert!(matches!(
        NoteSnapshot::capture(&clip, &[]),
        Err(ModelError::BadSong(_))
    ));
    assert!(matches!(
        NoteSnapshot::capture(&clip, &[5]),
        Err(ModelError::BadSong(_))
    ));
}

#[test]
fn pasting_notes_lands_at_a_song_tick_through_the_clip_start_and_offset() {
    let r = registry();
    let mut d = arranged(&r);
    let target = ids(&d)[1]; // starts at 3 BAR, one note at 0
    let src = clip_of(&d, ids(&d)[0]);
    let s = NoteSnapshot::capture(&src, &[0, 1]).unwrap();
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteNotes {
            clip: target,
            at_tick: 3 * BAR + PPQ,
            notes: s.clone(),
        }),
    );
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert_eq!(
        clip_notes(&d, target),
        [(0, PPQ, 67), (PPQ, PPQ, 60), (2 * PPQ, PPQ, 64)]
    );
    // A clip with a content offset shows content from the offset: the same song tick maps to
    // a later content tick.
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: target,
            start: 3 * BAR,
            length: 2 * BAR,
            offset: PPQ,
            loop_length: None,
            muted: false,
            merge: None,
        }),
        &r,
    )
    .unwrap();
    d.apply(
        wf(WorkflowCommand::PasteNotes {
            clip: target,
            at_tick: 3 * BAR,
            notes: s,
        }),
        &r,
    )
    .unwrap();
    // Pasted at content tick PPQ: the existing (PPQ, 60) is replaced, (2PPQ, 64) too.
    assert_eq!(
        clip_notes(&d, target),
        [(0, PPQ, 67), (PPQ, PPQ, 60), (2 * PPQ, PPQ, 64)]
    );
}

#[test]
fn a_pasted_note_replaces_one_with_the_same_start_and_key() {
    let r = registry();
    let mut d = arranged(&r);
    let id = ids(&d)[0];
    let clip = clip_of(&d, id);
    // Copy note 0 (60, length PPQ), make a longer twin by pasting a hand-made snapshot.
    let mut s = NoteSnapshot::capture(&clip, &[0]).unwrap();
    s.notes[0].length = 3 * PPQ;
    d.apply(
        wf(WorkflowCommand::PasteNotes {
            clip: id,
            at_tick: BAR,
            notes: s,
        }),
        &r,
    )
    .unwrap();
    assert_eq!(clip_notes(&d, id), [(0, 3 * PPQ, 60), (PPQ, PPQ, 64)]);
}

#[test]
fn paste_notes_rejects_a_negative_content_tick_and_non_note_clips() {
    let r = registry();
    let mut d = arranged(&r);
    let id = ids(&d)[0];
    let s = NoteSnapshot::capture(&clip_of(&d, id), &[0]).unwrap();
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteNotes {
            clip: id,
            at_tick: 0, // before the clip start (BAR): content tick -BAR
            notes: s.clone(),
        }),
    );
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteNotes {
            clip: ClipId(99),
            at_tick: BAR,
            notes: s.clone(),
        }),
    );
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
    run(
        &mut d,
        &r,
        [
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
            add_clip(
                "synth",
                1,
                auto_clip(0, BAR, vec![point(0, 500.0, Curve::Linear)]),
            ),
        ],
    );
    let auto = d.last_clip_id().unwrap();
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteNotes {
            clip: auto,
            at_tick: 0,
            notes: s,
        }),
    );
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn duplicating_notes_puts_the_copies_right_after_the_selection() {
    let r = registry();
    let mut d = arranged(&r);
    let id = ids(&d)[0];
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateNotes {
            clip: id,
            indices: vec![0, 1],
        }),
    );
    // The selection spans 0..2PPQ: the copies start at 2PPQ.
    assert_eq!(
        clip_notes(&d, id),
        [
            (0, PPQ, 60),
            (PPQ, PPQ, 64),
            (2 * PPQ, PPQ, 60),
            (3 * PPQ, PPQ, 64)
        ]
    );
    // One selected note: after that note only.
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateNotes {
            clip: id,
            indices: vec![0],
        }),
    );
    let n = clip_notes(&d, id);
    assert!(n.contains(&(PPQ, PPQ, 60)));
}

#[test]
fn duplicating_notes_rejects_bad_indices() {
    let r = registry();
    let mut d = arranged(&r);
    let id = ids(&d)[0];
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateNotes {
            clip: id,
            indices: vec![9],
        }),
    );
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicateNotes {
            clip: id,
            indices: Vec::new(),
        }),
    );
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

// ---------------------------------------------------------------- automation points

fn automation_doc(r: &atomik_device_sdk::Registry) -> (Document, ClipId) {
    let mut d = arranged(r);
    run(
        &mut d,
        r,
        [
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
            add_clip(
                "synth",
                1,
                auto_clip(
                    BAR,
                    2 * BAR,
                    vec![
                        point(0, 500.0, Curve::Linear),
                        point(PPQ, 800.0, Curve::Step),
                        point(2 * PPQ, 1200.0, Curve::Linear),
                    ],
                ),
            ),
        ],
    );
    let id = d.last_clip_id().unwrap();
    (d, id)
}

fn clip_points(d: &Document, id: ClipId) -> Vec<(i64, u32, Curve)> {
    match &clip_of(d, id).content {
        ClipContent::Automation(p) => p
            .iter()
            .map(|x| (x.tick, x.value as u32, x.curve))
            .collect(),
        _ => panic!("automation"),
    }
}

#[test]
fn points_paste_relative_to_the_earliest_and_duplicate_after_the_selection() {
    let r = registry();
    let (mut d, id) = automation_doc(&r);
    let s = PointSnapshot::capture(&clip_of(&d, id), &[1, 2]).unwrap();
    assert_eq!((s.points[0].tick, s.points[1].tick, s.span), (0, PPQ, PPQ));
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteAutomationPoints {
            clip: id,
            at_tick: BAR + 4 * PPQ,
            points: s,
        }),
    );
    let got = clip_points(&d, id);
    assert_eq!(got.len(), 5);
    assert_eq!(got[3], (4 * PPQ, 800, Curve::Step));
    assert_eq!(got[4], (5 * PPQ, 1200, Curve::Linear));
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DuplicatePoints {
            clip: id,
            indices: vec![0, 1],
        }),
    );
    // Selection 0..PPQ: copies at PPQ (replaces the point there) and 2PPQ (replaces).
    let got = clip_points(&d, id);
    assert_eq!(got[1], (PPQ, 500, Curve::Linear));
    assert_eq!(got[2], (2 * PPQ, 800, Curve::Step));
}

#[test]
fn points_do_not_paste_into_a_note_clip_and_bad_selections_are_rejected() {
    let r = registry();
    let (mut d, id) = automation_doc(&r);
    let note_clip = ids(&d)[0];
    let s = PointSnapshot::capture(&clip_of(&d, id), &[0]).unwrap();
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::PasteAutomationPoints {
            clip: note_clip,
            at_tick: BAR,
            points: s,
        }),
    );
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    assert!(matches!(
        PointSnapshot::capture(&clip_of(&d, id), &[7]),
        Err(ModelError::BadSong(_))
    ));
    assert!(matches!(
        PointSnapshot::capture(&clip_of(&d, note_clip), &[0]),
        Err(ModelError::BadSong(_))
    ));
}

// ---------------------------------------------------------------- neighbours

#[test]
fn neighbour_helpers_walk_devices_clips_and_tracks() {
    let r = registry();
    let d = arranged(&r);
    let doc = d.doc();
    assert_eq!(neighbour_device(doc, &key("synth"), 1), Some(key("pad")));
    assert_eq!(neighbour_device(doc, &key("pad"), -1), Some(key("synth")));
    assert_eq!(neighbour_device(doc, &key("synth"), -1), None);
    assert_eq!(neighbour_device(doc, &key("pad"), 1), None);
    assert_eq!(neighbour_device(doc, &key("ghost"), 1), None);
    // Inner devices walk inside their container.
    let c = container_doc(&r);
    assert_eq!(
        neighbour_device(c.doc(), &key("c/synth"), 1),
        Some(key("c/delay"))
    );
    assert_eq!(neighbour_device(c.doc(), &key("c/delay"), 1), None);
    // Clips in time order across tracks: synth BAR, pad 2BAR, synth 3BAR.
    let all = ids(&d);
    assert_eq!(neighbour_clip(&doc.song, all[0], true), Some(all[2]));
    assert_eq!(neighbour_clip(&doc.song, all[2], true), Some(all[1]));
    assert_eq!(neighbour_clip(&doc.song, all[1], true), None);
    assert_eq!(neighbour_clip(&doc.song, all[1], false), Some(all[2]));
    assert_eq!(neighbour_clip(&doc.song, all[0], false), None);
    assert_eq!(neighbour_clip(&doc.song, ClipId(500), true), None);
    // Tracks in song order.
    assert_eq!(
        neighbour_track(&doc.song, &key("synth"), 1),
        Some(key("pad"))
    );
    assert_eq!(neighbour_track(&doc.song, &key("pad"), 1), None);
    assert_eq!(
        neighbour_track(&doc.song, &key("pad"), -1),
        Some(key("synth"))
    );
}

// ---------------------------------------------------------------- more edges

#[test]
fn a_hand_made_empty_clip_snapshot_is_rejected() {
    let r = registry();
    let mut d = arranged(&r);
    let mut s = ClipSnapshot::capture(d.doc(), &ids(&d)).unwrap();
    s.entries.clear();
    let e = rejected(&mut d, &r, paste_clips(None, 0, &s));
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn clip_ids_are_allocated_from_the_next_free_id_in_snapshot_order() {
    let r = registry();
    let mut d = arranged(&r);
    // Remove the newest clip: its id is never reused (next_clip_id only grows).
    let all = ids(&d);
    let next = d.doc().song.next_clip_id;
    d.apply(
        sc(SongCommand::RemoveClip {
            id: *all.last().unwrap(),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(d.doc().song.next_clip_id, next);
    let s = ClipSnapshot::capture(d.doc(), &ids(&d)).unwrap();
    d.apply(paste_clips(None, 30 * BAR, &s), &r).unwrap();
    let new: Vec<u64> = ids(&d).iter().map(|i| i.0).filter(|i| *i >= next).collect();
    assert_eq!(new, [next, next + 1]);
}

#[test]
fn pasting_notes_leaves_the_clip_geometry_alone() {
    let r = registry();
    let mut d = arranged(&r);
    let id = ids(&d)[0];
    let before = clip_of(&d, id);
    let s = NoteSnapshot::capture(&before, &[0]).unwrap();
    d.apply(
        wf(WorkflowCommand::PasteNotes {
            clip: id,
            at_tick: BAR + 3 * BAR, // far past the clip end
            notes: s,
        }),
        &r,
    )
    .unwrap();
    let after = clip_of(&d, id);
    assert_eq!(
        (after.start, after.length, after.offset),
        (before.start, before.length, before.offset)
    );
}

// ---------------------------------------------------------------- remove clips

#[test]
fn removing_several_clips_is_one_undo_step_and_an_unknown_clip_changes_nothing() {
    let r = registry();
    let mut d = arranged(&r);
    let all = ids(&d);
    let before = text(&d);
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveClips {
            ids: vec![all[2], all[0]],
        }),
    );
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert_eq!(ids(&d), [all[1]]);
    assert!(d.undo(&r).is_some());
    assert_eq!(text(&d), before);
    assert!(d.redo(&r).is_some());
    assert_eq!(ids(&d), [all[1]]);
    // A clip that does not exist, or no clips at all: rejected, nothing changed.
    let now = text(&d);
    let bad = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveClips {
            ids: vec![all[1], ClipId(999)],
        }),
    );
    assert!(matches!(bad, ModelError::UnknownKey(_)), "{bad:?}");
    let none = rejected(&mut d, &r, wf(WorkflowCommand::RemoveClips { ids: vec![] }));
    assert!(matches!(none, ModelError::BadSong(_)), "{none:?}");
    assert_eq!(text(&d), now);
}

#[test]
fn a_joined_clip_is_muted_only_when_every_part_was_muted() {
    let r = registry();
    let mute = |d: &mut Document, id: ClipId, muted: bool| {
        let c = clip_of(d, id);
        d.apply(
            sc(SongCommand::SetClipGeometry {
                id,
                start: c.start,
                length: c.length,
                offset: c.offset,
                loop_length: c.loop_length,
                muted,
                merge: None,
            }),
            &r,
        )
        .unwrap();
    };
    let synth_clips = |d: &Document| -> Vec<ClipId> {
        d.doc().song.track(&key("synth")).unwrap().lanes[0]
            .clips
            .iter()
            .map(|c| c.id)
            .collect()
    };
    let mut d = arranged(&r);
    let two = synth_clips(&d);
    mute(&mut d, two[0], true);
    let join = wf(WorkflowCommand::JoinClips { ids: two.clone() });
    let e = rejected(&mut d, &r, join.clone());
    assert!(
        matches!(e, ModelError::BadSong(_)),
        "one audible part is not enough to join: {e:?}"
    );
    mute(&mut d, two[1], true);
    one_step(&mut d, &r, join);
    assert!(
        clip_of(&d, two[0]).muted,
        "all parts muted: the join is muted"
    );
}
