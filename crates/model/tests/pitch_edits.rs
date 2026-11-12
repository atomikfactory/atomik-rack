//! Pitch edits of audio clips (the Vocal mode of the audio clip editor): the command, the file,
//! the split and the join, and the render job.

mod common;

use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use atomik_model::{
    AudioClipContent, ClipContent, ClipId, Command, Document, PitchEdit, SongCommand,
    WorkflowCommand, load_project, save_project, warp_job,
};
use common::audio::*;
use common::song::*;
use common::workflow::wf;

/// A clip of 16 beats (8 s, 384 000 frames at 120 BPM) on asset 1 (480 000 frames).
fn one_clip(reg: &Registry) -> (Document, ClipId) {
    let mut d = with_audio_track(reg);
    d.apply(add_audio_clip("audio-track", 0, 16 * PPQ, 1), reg)
        .unwrap();
    let id = last(&d);
    (d, id)
}

fn content(d: &Document, id: ClipId) -> AudioClipContent {
    match &doc_clip(d.doc(), id).content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

fn edit(a: u64, b: u64, cents: i32) -> PitchEdit {
    PitchEdit {
        shift_cents: cents,
        ..PitchEdit::new(a, b)
    }
}

fn set(id: ClipId, edits: Vec<PitchEdit>, merge: Option<u64>) -> Command {
    sc(SongCommand::SetPitchEdits { id, edits, merge })
}

#[test]
fn set_pitch_edits_is_one_undo_step_and_keeps_the_rest_of_the_content() {
    let reg = audio_registry();
    let (mut d, id) = one_clip(&reg);
    let before = d.doc().clone();
    let list = vec![edit(1000, 20_000, 300), edit(30_000, 40_000, -50)];
    d.apply(set(id, list.clone(), None), &reg).unwrap();
    let a = content(&d, id);
    assert_eq!(a.pitch_edits, list);
    assert_eq!(a.warp, before_warp(&before, id));
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    d.redo().unwrap();
    assert_eq!(content(&d, id).pitch_edits, list);
}

fn before_warp(doc: &atomik_model::RackDoc, id: ClipId) -> atomik_model::WarpSettings {
    match &doc_clip(doc, id).content {
        ClipContent::Audio(a) => a.warp.clone(),
        _ => panic!("not audio"),
    }
}

#[test]
fn a_pitch_drag_is_one_merge_group() {
    let reg = audio_registry();
    let (mut d, id) = one_clip(&reg);
    let before = d.doc().clone();
    for cents in [100, 200, 300] {
        d.apply(set(id, vec![edit(1000, 20_000, cents)], Some(9)), &reg)
            .unwrap();
    }
    assert_eq!(content(&d, id).pitch_edits[0].shift_cents, 300);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn bad_pitch_edits_are_refused() {
    let reg = audio_registry();
    let (mut d, id) = one_clip(&reg);
    // Equal to the current list, overlapping, shorter than 10 ms, past the asset end.
    for bad in [
        Vec::new(),
        vec![edit(0, 1000, 10), edit(500, 2000, 10)],
        vec![edit(0, 100, 10)],
        vec![edit(479_000, 480_001, 10)],
        vec![edit(0, 1000, 2401)],
    ] {
        let before = d.doc().clone();
        assert!(d.apply(set(id, bad, None), &reg).is_err());
        assert_eq!(d.doc(), &before);
    }
}

#[test]
fn a_split_keeps_each_edit_in_the_piece_that_plays_its_start() {
    let reg = audio_registry();
    let (mut d, id) = one_clip(&reg);
    d.apply(
        set(
            id,
            vec![edit(1000, 20_000, 100), edit(90_000, 110_000, 200)],
            None,
        ),
        &reg,
    )
    .unwrap();
    // A beat is 24 000 frames: the cut at beat 4 is source frame 96 000.
    d.apply(sc(SongCommand::SplitClip { id, at: 4 * PPQ }), &reg)
        .unwrap();
    let right = last(&d);
    assert_eq!(
        content(&d, id).pitch_edits,
        vec![edit(1000, 20_000, 100), edit(90_000, 96_000, 200)]
    );
    assert!(content(&d, right).pitch_edits.is_empty());
    // A join keeps both lists again.
    d.apply(
        sc(SongCommand::SetPitchEdits {
            id: right,
            edits: vec![edit(100_000, 120_000, -300)],
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        wf(WorkflowCommand::JoinClips {
            ids: vec![id, right],
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(
        content(&d, id).pitch_edits,
        vec![
            edit(1000, 20_000, 100),
            edit(90_000, 96_000, 200),
            edit(100_000, 120_000, -300)
        ]
    );
}

#[test]
fn the_file_keeps_the_edits_and_writes_nothing_without_them() {
    let reg = audio_registry();
    let (mut d, id) = one_clip(&reg);
    let plain = save_project(d.doc());
    assert!(!plain.contains("\"pitch_edits\": ["));
    let edits = vec![
        edit(1000, 20_000, 100),
        PitchEdit {
            drift: 80,
            vibrato_keep: false,
            ..edit(30_000, 40_000, -1250)
        },
    ];
    d.apply(set(id, edits.clone(), None), &reg).unwrap();
    let text = save_project(d.doc());
    assert!(text.contains(
        "\"pitch_edits\": [[1000, 20000, 100, 0, true], [30000, 40000, -1250, 80, false]]"
    ));
    let back = load_project(&text, &reg).unwrap();
    assert_eq!(&back, d.doc());
    // The edits are an ignorable field of format 45.
    assert!(text.contains("\"song.tracks[].lanes[].clips[].audio.pitch_edits\": 45"));
    // A file whose edits overlap is refused with a path.
    let broken = text.replace("[30000, 40000", "[10000, 40000");
    let err = load_project(&broken, &reg).unwrap_err().to_string();
    assert!(err.contains("pitch_edits"), "{err}");
}

#[test]
fn edits_make_a_render_job_and_change_its_key() {
    let reg = audio_registry();
    let (mut d, id) = one_clip(&reg);
    let job = |d: &Document| {
        let c = doc_clip(d.doc(), id);
        let a = content(d, id);
        warp_job(&d.doc().song, 48_000, c.start, c.length, &a)
    };
    // Warp off and no edits: no job; a neutral edit makes none either.
    assert!(job(&d).is_none());
    d.apply(set(id, vec![edit(1000, 20_000, 0)], None), &reg)
        .unwrap();
    assert!(job(&d).is_none());
    // An edit that shifts gives a 1:1 job carrying it.
    d.apply(set(id, vec![edit(1000, 20_000, 300)], None), &reg)
        .unwrap();
    let a = job(&d).expect("a clip with edits is rendered");
    assert_eq!(a.pitch, vec![edit(1000, 20_000, 300)]);
    assert_eq!(a.knots.first(), Some(&(0.0, 0.0)));
    d.apply(set(id, vec![edit(1000, 20_000, 400)], None), &reg)
        .unwrap();
    let b = job(&d).unwrap();
    assert_ne!(a.key(), b.key());
    let drift = PitchEdit {
        drift: 50,
        ..edit(1000, 20_000, 400)
    };
    d.apply(set(id, vec![drift], None), &reg).unwrap();
    assert_ne!(job(&d).unwrap().key(), b.key());
}

#[test]
fn copied_clips_keep_their_edits_through_the_clipboard_text() {
    let reg = audio_registry();
    let (mut d, id) = one_clip(&reg);
    d.apply(set(id, vec![edit(1000, 20_000, -700)], None), &reg)
        .unwrap();
    let snapshot = atomik_model::ClipSnapshot::capture(d.doc(), &[id]).unwrap();
    let text = atomik_model::clips_to_text(&snapshot);
    let back = atomik_model::clips_from_text(&text).unwrap();
    assert_eq!(back, snapshot);
    let ClipContent::Audio(a) = &back.entries[0].clip.content else {
        panic!("audio")
    };
    assert_eq!(a.pitch_edits, vec![edit(1000, 20_000, -700)]);
}
