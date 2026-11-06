//! Restructuring follow-ups (model half): Extend to Next and Previous Clip on audio clips, and
//! the clips of a range that spans several lanes.

mod common;

use atomik_model::{
    AudioClipContent, Clip, ClipContent, ClipId, ClipSnapshot, Command, Curve, Document,
    ModelError, SongCommand, WorkflowCommand,
};
use common::audio::*;
use common::song::*;
use common::workflow::{one_step, wf};
use common::*;

/// 120 BPM at 48 kHz: a beat (PPQ ticks) is 24 000 frames.
const BEAT_F: u64 = 24_000;

fn setup() -> (atomik_device_sdk::Registry, Document) {
    let reg = audio_registry();
    let d = with_audio_track(&reg);
    (reg, d)
}

fn put(
    d: &mut Document,
    reg: &atomik_device_sdk::Registry,
    start: i64,
    len: i64,
    a: AudioClipContent,
) -> ClipId {
    d.apply(
        add_audio_clip_with("audio-track", start, len, a.clone()),
        reg,
    )
    .unwrap();
    let id = last(d);
    let c = doc_clip(d.doc(), id).clone();
    d.apply(
        sc(SongCommand::SetAudioClip {
            id,
            start: c.start,
            length: c.length,
            muted: c.muted,
            content: a,
            merge: None,
        }),
        reg,
    )
    .unwrap();
    id
}

fn clip(d: &Document, id: ClipId) -> Clip {
    doc_clip(d.doc(), id).clone()
}

fn audio_of(d: &Document, id: ClipId) -> AudioClipContent {
    match &doc_clip(d.doc(), id).content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

fn extend(ids: &[ClipId], next: bool) -> Command {
    wf(WorkflowCommand::ExtendClips {
        ids: ids.to_vec(),
        next,
    })
}

fn from(frames_in_beats: u64, a: AudioClipContent) -> AudioClipContent {
    AudioClipContent {
        source_offset: frames_in_beats * BEAT_F,
        ..a
    }
}

#[test]
fn an_audio_clip_extends_to_the_next_clip_and_keeps_its_fades_and_offset() {
    let (reg, mut d) = setup();
    let a = put(
        &mut d,
        &reg,
        0,
        2 * PPQ,
        AudioClipContent {
            fade_in: 1000,
            fade_out: 2000,
            ..audio(1)
        },
    );
    let b = put(&mut d, &reg, 6 * PPQ, PPQ, audio(2));
    let before = audio_of(&d, a);
    one_step(&mut d, &reg, extend(&[a], true));
    let c = clip(&d, a);
    assert_eq!((c.start, c.end()), (0, 6 * PPQ));
    assert_eq!(audio_of(&d, a), before, "the sound and the fades stay");
    assert_eq!(clip(&d, b).start, 6 * PPQ);
    d.undo().unwrap();
    assert_eq!(clip(&d, a).length, 2 * PPQ);
}

#[test]
fn an_audio_clip_extends_back_only_as_far_as_its_file_goes() {
    let (reg, mut d) = setup();
    let first = put(&mut d, &reg, 0, PPQ, audio(1));
    // Two beats of the file precede this clip's first frame, so it can reach back two beats:
    // the clip before ends four beats earlier, which is out of reach.
    let b = put(&mut d, &reg, 6 * PPQ, PPQ, from(2, audio(2)));
    one_step(&mut d, &reg, extend(&[b], false));
    let c = clip(&d, b);
    assert_eq!((c.start, c.end()), (4 * PPQ, 7 * PPQ));
    assert_eq!(audio_of(&d, b).source_offset, 0, "the whole lead-in plays");
    // Nothing is left to play before the first frame: the same command now refuses.
    let err = d.apply(extend(&[b], false), &reg).unwrap_err();
    assert!(matches!(err, ModelError::BadSong(_)), "{err:?}");
    assert_eq!(clip(&d, first).start, 0);
}

#[test]
fn an_audio_clip_with_a_full_lead_in_reaches_the_previous_clip() {
    let (reg, mut d) = setup();
    put(&mut d, &reg, 0, 2 * PPQ, audio(1));
    let b = put(&mut d, &reg, 6 * PPQ, PPQ, from(4, audio(2)));
    one_step(&mut d, &reg, extend(&[b], false));
    let c = clip(&d, b);
    assert_eq!((c.start, c.end()), (2 * PPQ, 7 * PPQ));
    assert_eq!(audio_of(&d, b).source_offset, 0);
}

#[test]
fn a_reversed_audio_clip_moves_its_source_offset_with_its_end() {
    let (reg, mut d) = setup();
    // The clip plays frames [4 beats, 6 beats) backwards: its end is the low end of that span.
    let a = put(
        &mut d,
        &reg,
        0,
        2 * PPQ,
        AudioClipContent {
            reverse: true,
            ..from(4, audio(1))
        },
    );
    put(&mut d, &reg, 12 * PPQ, PPQ, audio(2));
    one_step(&mut d, &reg, extend(&[a], true));
    let c = clip(&d, a);
    // The file holds four more beats below the offset: the clip grows by four, not ten.
    assert_eq!((c.start, c.end()), (0, 6 * PPQ));
    let content = audio_of(&d, a);
    assert!(content.reverse);
    assert_eq!(content.source_offset, 0);
}

#[test]
fn a_selection_of_notes_and_audio_extends_in_one_undo_step() {
    let (reg, mut d) = setup();
    let a = put(&mut d, &reg, 0, PPQ, audio(1));
    put(&mut d, &reg, 3 * PPQ, PPQ, audio(2));
    run(
        &mut d,
        &reg,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip("synth", 0, notes_clip(0, PPQ, vec![note(0, PPQ, 60, 0.8)])),
        ],
    );
    let n = last(&d);
    run(
        &mut d,
        &reg,
        [add_clip(
            "synth",
            0,
            notes_clip(4 * PPQ, PPQ, vec![note(0, PPQ, 62, 0.8)]),
        )],
    );
    let depth = d.undo_depth();
    one_step(&mut d, &reg, extend(&[a, n], true));
    assert_eq!(d.undo_depth(), depth + 1, "one undo step for both kinds");
    assert_eq!(clip(&d, a).end(), 3 * PPQ);
    assert_eq!(clip(&d, n).end(), 4 * PPQ);
    d.undo().unwrap();
    assert_eq!(clip(&d, a).end(), PPQ);
    assert_eq!(clip(&d, n).end(), PPQ);
}

// -------------------------------------------------------------------- a range over lanes

#[test]
fn a_range_over_several_lanes_copies_the_parts_of_every_lane() {
    let (reg, mut d) = setup();
    let a = put(&mut d, &reg, 0, 4 * PPQ, audio(1));
    let doc = d.doc();
    let track = doc.song.tracks[0].device.clone();
    let snap = ClipSnapshot::capture_ranges(doc, &[(track.clone(), 0)], (PPQ, 3 * PPQ)).unwrap();
    assert_eq!(snap.entries.len(), 1);
    assert_eq!((snap.origin, snap.span), (PPQ, 2 * PPQ));
    assert_eq!(snap.entries[0].clip.start, 0);
    assert_eq!(snap.entries[0].clip.length, 2 * PPQ);
    // The same lane listed twice is read once.
    let twice = ClipSnapshot::capture_ranges(
        doc,
        &[(track.clone(), 0), (track.clone(), 0)],
        (PPQ, 3 * PPQ),
    )
    .unwrap();
    assert_eq!(twice, snap);
    // A lane that does not exist refuses; so does a range that holds nothing.
    assert!(ClipSnapshot::capture_ranges(doc, &[(track.clone(), 9)], (0, PPQ)).is_err());
    assert!(ClipSnapshot::capture_ranges(doc, &[(track, 0)], (8 * PPQ, 9 * PPQ)).is_err());
    let _ = a;
}

#[test]
fn a_range_over_a_note_lane_and_an_automation_lane_keeps_each_lane_kind() {
    let (reg, mut d) = setup();
    run(
        &mut d,
        &reg,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
            add_clip(
                "synth",
                0,
                notes_clip(
                    0,
                    4 * PPQ,
                    vec![note(0, PPQ, 60, 0.8), note(2 * PPQ, PPQ, 62, 0.8)],
                ),
            ),
            add_clip(
                "synth",
                1,
                auto_clip(
                    PPQ,
                    4 * PPQ,
                    vec![point(0, 0.2, Curve::Linear), point(PPQ, 0.8, Curve::Linear)],
                ),
            ),
        ],
    );
    let doc = d.doc();
    let lanes = [(key("synth"), 0), (key("synth"), 1)];
    let snap = ClipSnapshot::capture_ranges(doc, &lanes, (PPQ, 3 * PPQ)).unwrap();
    assert_eq!(snap.entries.len(), 2, "one part from each lane");
    assert_eq!((snap.origin, snap.span), (PPQ, 2 * PPQ));
    assert!(snap.entries.iter().any(|e| e.lane.is_notes()));
    assert!(
        snap.entries
            .iter()
            .any(|e| !e.lane.is_notes() && !e.lane.is_audio())
    );
    // A lane with nothing in the range adds nothing; the other still copies.
    let only = ClipSnapshot::capture_ranges(doc, &lanes, (0, PPQ)).unwrap();
    assert_eq!(only.entries.len(), 1);
    // Pasting puts each part back on a lane of its own kind, in one undo step.
    let depth = d.undo_depth();
    one_step(
        &mut d,
        &reg,
        wf(WorkflowCommand::PasteClips {
            track: None,
            lane: None,
            at_tick: 8 * PPQ,
            clips: snap,
        }),
    );
    assert_eq!(d.undo_depth(), depth + 1);
    let t = d.doc().song.track(&key("synth")).unwrap();
    assert_eq!(t.lanes[0].clips.len(), 2);
    assert_eq!(t.lanes[1].clips.len(), 2);
    assert!(
        t.lanes
            .iter()
            .all(|l| l.clips.iter().any(|c| c.start == 8 * PPQ))
    );
}
