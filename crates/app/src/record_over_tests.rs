//! Record mode and keep-events policy: names, cycling, the take command each mode picks and the
//! follow-up commands of each policy.

use super::*;
use atomik_core::PPQ;
use atomik_model::song::{Lane, Track, empty_song};
use atomik_model::{Clip, ClipContent, ClipId, DeviceKey, Note, empty_doc};

const BAR: i64 = 4 * PPQ;

fn clip(start: i64, length: i64, notes: Vec<Note>) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        content: ClipContent::Notes(notes),
    }
}

fn doc_with(clips: Vec<Clip>) -> RackDoc {
    let mut doc = empty_doc();
    doc.song = empty_song(120.0);
    let clips = clips
        .into_iter()
        .enumerate()
        .map(|(i, c)| Clip {
            id: ClipId(i as u64 + 1),
            ..c
        })
        .collect();
    doc.song.tracks.push(Track {
        device: DeviceKey("a".into()),
        muted: false,
        solo: false,
        armed: false,
        chase: true,
        notes_to: None,
        frozen: None,
        lanes: vec![Lane::notes().with_clips(clips)],
    });
    doc
}

#[test]
fn the_names_round_trip_and_the_choices_cycle() {
    for m in RecordMode::ALL {
        assert_eq!(RecordMode::from_id(m.id()), Some(m));
    }
    for p in KeepEvents::ALL {
        assert_eq!(KeepEvents::from_id(p.id()), Some(p));
    }
    assert_eq!(RecordMode::from_id("nope"), None);
    assert_eq!(RecordMode::default(), RecordMode::Replace);
    assert_eq!(KeepEvents::default(), KeepEvents::Keep);
    assert_eq!(RecordMode::Replace.next(), RecordMode::Layer);
    assert_eq!(RecordMode::Layer.next(), RecordMode::Replace);
    let mut p = KeepEvents::Keep;
    let seen: Vec<_> = (0..4)
        .map(|_| {
            p = p.next();
            p
        })
        .collect();
    assert_eq!(
        seen,
        [
            KeepEvents::Expand,
            KeepEvents::NewClip,
            KeepEvents::Keep,
            KeepEvents::Expand
        ]
    );
}

#[test]
fn replace_writes_a_clip_or_a_take_group_and_layer_merges_or_engulfs() {
    let device = DeviceKey("a".into());
    let take = |start: i64| clip(start, BAR, vec![Note::new(0, PPQ, 60, 0.8)]);
    let empty = doc_with(Vec::new());
    let cmd = |doc: &RackDoc, laps: Vec<Clip>, mode| {
        note_take_command(doc, device.clone(), 0, laps, mode)
    };
    assert!(matches!(
        cmd(&empty, vec![take(0)], RecordMode::Replace),
        Command::Song(SongCommand::AddClip { .. })
    ));
    assert!(matches!(
        cmd(&empty, vec![take(0), take(0)], RecordMode::Replace),
        Command::Song(SongCommand::NewTakeGroup { .. })
    ));
    // Layer: nothing under the take: it is added over what starts inside it.
    assert!(matches!(
        cmd(&empty, vec![take(0), take(0)], RecordMode::Layer),
        Command::Workflow(WorkflowCommand::EngulfRecording { .. })
    ));
    // A clip under the take's start: the take merges into it.
    let covered = doc_with(vec![clip(0, 2 * BAR, Vec::new())]);
    match cmd(&covered, vec![take(BAR)], RecordMode::Layer) {
        Command::Workflow(WorkflowCommand::LayerRecording { clip, .. }) => {
            assert_eq!(clip, ClipId(1));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn follow_ups_are_empty_for_keep_inside_notes_loops_and_other_clips() {
    let outside = vec![Note::new(0, PPQ, 60, 0.8), Note::new(BAR, PPQ, 61, 0.8)];
    let doc = doc_with(vec![clip(0, BAR, outside.clone())]);
    assert!(keep_events_follow_up(&doc, ClipId(1), KeepEvents::Keep).is_empty());
    assert!(keep_events_follow_up(&doc, ClipId(9), KeepEvents::Expand).is_empty());
    let inside = doc_with(vec![clip(0, BAR, vec![Note::new(BAR - 1, 1, 60, 0.8)])]);
    assert!(keep_events_follow_up(&inside, ClipId(1), KeepEvents::Expand).is_empty());
    let mut looped = clip(0, 2 * BAR, outside);
    looped.loop_length = Some(BAR);
    let doc = doc_with(vec![looped]);
    assert!(keep_events_follow_up(&doc, ClipId(1), KeepEvents::NewClip).is_empty());
}

#[test]
fn expand_goes_to_the_bar_line_after_the_farthest_note_end() {
    let doc = doc_with(vec![clip(
        BAR,
        BAR,
        vec![
            Note::new(0, PPQ, 60, 0.8),
            Note::new(BAR, PPQ, 61, 0.8),
            // Starts in the second bar and ends exactly on the third bar line.
            Note::new(BAR + 3 * PPQ, PPQ, 62, 0.8),
        ],
    )]);
    let commands = keep_events_follow_up(&doc, ClipId(1), KeepEvents::Expand);
    match commands.as_slice() {
        [Command::Song(SongCommand::SetClipGeometry { start, length, .. })] => {
            assert_eq!((*start, *length), (BAR, 2 * BAR))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn new_clip_starts_on_the_bar_line_but_never_before_the_old_clip_ends() {
    let doc = doc_with(vec![clip(
        0,
        BAR + PPQ,
        vec![
            Note::new(0, PPQ, 60, 0.8),
            Note::new(BAR + 2 * PPQ, PPQ, 61, 0.8),
        ],
    )]);
    let commands = keep_events_follow_up(&doc, ClipId(1), KeepEvents::NewClip);
    let [
        Command::Song(SongCommand::SetClipContent { content, .. }),
        Command::Song(SongCommand::AddClip { clip: new, .. }),
    ] = commands.as_slice()
    else {
        panic!("{commands:?}");
    };
    match content {
        ClipContent::Notes(n) => assert_eq!(n.len(), 1),
        _ => unreachable!(),
    }
    // The old clip ends a beat into bar 2 and the note is on its third beat: the bar line is
    // inside the old clip, so the new clip starts at the old clip's end (never before it), runs
    // to the next bar line and holds the note rebased.
    assert_eq!((new.start, new.length), (BAR + PPQ, BAR - PPQ));
    match &new.content {
        ClipContent::Notes(n) => assert_eq!(n[0].tick, PPQ),
        _ => unreachable!(),
    }
}
