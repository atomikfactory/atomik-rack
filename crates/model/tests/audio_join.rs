//! Joining audio clips and moving the crossfade between two of them.

mod common;

use atomik_model::song::{crossfades, flatten_audio};
use atomik_model::{
    AudioClipContent, ClipContent, ClipId, Command, Document, GainPoint, ModelError, SongCommand,
    WorkflowCommand,
};
use common::audio::*;
use common::song::*;
use common::workflow::wf;
use common::*;

const SR: u32 = 48_000;
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
    // `AddAudioClip` has no reverse, envelope or crossfade: set the whole content.
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

fn audio_of(d: &Document, id: ClipId) -> AudioClipContent {
    match &doc_clip(d.doc(), id).content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

fn join(
    d: &mut Document,
    reg: &atomik_device_sdk::Registry,
    ids: &[ClipId],
) -> Result<Command, ModelError> {
    let c = wf(WorkflowCommand::JoinClips { ids: ids.to_vec() });
    d.apply(c.clone(), reg).map(|_| c)
}

fn clips_on_lane(d: &Document) -> usize {
    d.doc().song.tracks[0].lanes[0].clips.len()
}

/// What plays at song frame `frame`: the asset and its source frame, summed over the regions
/// of the flattened lane that cover it (crossfade overlaps list both).
fn playing_at(d: &Document, frame: u64) -> Vec<(String, u64)> {
    let song = &d.doc().song;
    let mut out: Vec<(String, u64)> =
        flatten_audio(song.track(&key("audio-track")).unwrap(), song, SR)
            .into_iter()
            .filter(|r| (r.song_start_frame..r.song_start_frame + r.frames).contains(&frame))
            .map(|r| {
                (
                    r.asset.clone(),
                    r.source_offset + (frame - r.song_start_frame),
                )
            })
            .collect();
    out.sort();
    out
}

#[test]
fn a_split_and_a_join_give_the_clip_back() {
    let (reg, mut d) = setup();
    let id = put(&mut d, &reg, 0, 4 * PPQ, audio(1));
    let before = d.doc().song.clone();
    d.apply(sc(SongCommand::SplitClip { id, at: 2 * PPQ }), &reg)
        .unwrap();
    let right = last(&d);
    assert_eq!(clips_on_lane(&d), 2);
    join(&mut d, &reg, &[right, id]).unwrap();
    assert_eq!(clips_on_lane(&d), 1);
    let joined = doc_clip(d.doc(), id);
    assert_eq!((joined.start, joined.length), (0, 4 * PPQ));
    assert_eq!(audio_of(&d, id).source_offset, 0);
    assert_eq!(
        d.doc().song.tracks,
        before.tracks,
        "the joined clip is the one that was split"
    );
    // One undo step puts the two pieces back.
    d.undo().unwrap();
    assert_eq!(clips_on_lane(&d), 2);
}

#[test]
fn a_join_sounds_like_the_pieces() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        envelope: vec![
            GainPoint {
                frame: 0,
                gain_db: -12.0,
            },
            GainPoint {
                frame: 4 * BEAT_F,
                gain_db: 0.0,
            },
        ],
        gain_db: -3.0,
        ..audio(1)
    };
    let id = put(&mut d, &reg, 0, 4 * PPQ, a.clone());
    d.apply(sc(SongCommand::SplitClip { id, at: PPQ }), &reg)
        .unwrap();
    let second = last(&d);
    d.apply(
        sc(SongCommand::SplitClip {
            id: second,
            at: 3 * PPQ,
        }),
        &reg,
    )
    .unwrap();
    let third = last(&d);
    assert_eq!(clips_on_lane(&d), 3);
    join(&mut d, &reg, &[third, id, second]).unwrap();
    assert_eq!(clips_on_lane(&d), 1);
    let joined = audio_of(&d, id);
    assert_eq!(joined.gain_db, -3.0);
    for n in [
        0,
        1,
        BEAT_F - 1,
        BEAT_F,
        2 * BEAT_F,
        3 * BEAT_F - 1,
        3 * BEAT_F,
        4 * BEAT_F - 1,
    ] {
        let want = a.envelope_gain_at(n).unwrap();
        let got = joined.envelope_gain_at(n).unwrap();
        assert!((got - want).abs() < 1e-4, "frame {n}: {got} vs {want}");
    }
    assert_eq!(
        playing_at(&d, 2 * BEAT_F + 7),
        vec![(hash(1), 2 * BEAT_F + 7)]
    );
}

#[test]
fn pieces_of_different_files_are_refused_with_their_names() {
    let (reg, mut d) = setup();
    let a = put(&mut d, &reg, 0, 2 * PPQ, audio(1));
    let b = put(&mut d, &reg, 2 * PPQ, PPQ, audio(2));
    let err = join(&mut d, &reg, &[a, b]).unwrap_err().to_string();
    assert!(err.contains("different files"), "{err}");
    assert!(
        err.contains("take-480000.wav") && err.contains("take-96000.wav"),
        "{err}"
    );
    assert_eq!(clips_on_lane(&d), 2);
}

#[test]
fn a_gap_a_jump_in_the_file_and_other_differences_are_refused() {
    let (reg, mut d) = setup();
    let a = put(&mut d, &reg, 0, PPQ, audio(1));
    let gap = put(&mut d, &reg, 2 * PPQ, PPQ, audio(1));
    let err = join(&mut d, &reg, &[a, gap]).unwrap_err().to_string();
    assert!(err.contains("does not touch"), "{err}");
    // Touching, but playing another part of the file.
    let jump = put(
        &mut d,
        &reg,
        PPQ,
        PPQ,
        AudioClipContent {
            source_offset: 3 * BEAT_F,
            ..audio(1)
        },
    );
    let err = join(&mut d, &reg, &[a, jump]).unwrap_err().to_string();
    assert!(err.contains("does not continue"), "{err}");
    // The right place in the file, another gain.
    d.apply(sc(SongCommand::RemoveClip { id: jump }), &reg)
        .unwrap();
    let loud = put(
        &mut d,
        &reg,
        PPQ,
        PPQ,
        AudioClipContent {
            source_offset: BEAT_F,
            gain_db: 6.0,
            ..audio(1)
        },
    );
    let err = join(&mut d, &reg, &[a, loud]).unwrap_err().to_string();
    assert!(err.contains("gain or warp"), "{err}");
    // A fade at the join would be lost.
    d.apply(sc(SongCommand::RemoveClip { id: loud }), &reg)
        .unwrap();
    let faded = put(
        &mut d,
        &reg,
        PPQ,
        PPQ,
        AudioClipContent {
            source_offset: BEAT_F,
            fade_in: 100,
            ..audio(1)
        },
    );
    let err = join(&mut d, &reg, &[a, faded]).unwrap_err().to_string();
    assert!(err.contains("fades in"), "{err}");
    // A reversed clip never joins.
    let rev = put(
        &mut d,
        &reg,
        5 * PPQ,
        PPQ,
        AudioClipContent {
            reverse: true,
            ..audio(1)
        },
    );
    let err = join(&mut d, &reg, &[a, rev]).unwrap_err().to_string();
    assert!(err.contains("backwards"), "{err}");
}

/// Two clips of file 1 that overlap by a beat: the later one starts at beat 2 of the earlier
/// (which is three beats long) and plays on from the file frame it would have reached.
fn crossfade_pair(d: &mut Document, reg: &atomik_device_sdk::Registry) -> (ClipId, ClipId) {
    let earlier = put(d, reg, 0, 3 * PPQ, audio(1));
    let later = put(
        d,
        reg,
        2 * PPQ,
        3 * PPQ,
        AudioClipContent {
            source_offset: 2 * BEAT_F,
            ..audio(1)
        },
    );
    (earlier, later)
}

fn move_xfade(
    d: &mut Document,
    reg: &atomik_device_sdk::Registry,
    later: ClipId,
    delta: i64,
) -> Result<(), ModelError> {
    d.apply(wf(WorkflowCommand::MoveCrossfade { later, delta }), reg)
        .map(|_| ())
}

#[test]
fn moving_a_crossfade_moves_the_cut_and_keeps_the_sound_outside_it() {
    let (reg, mut d) = setup();
    let (earlier, later) = crossfade_pair(&mut d, &reg);
    let zone = crossfades(&d.doc().song.tracks[0].lanes[0])[0];
    assert_eq!((zone.start, zone.end), (2 * PPQ, 3 * PPQ));
    let before = d.doc().song.clone();
    let delta = PPQ / 4;
    move_xfade(&mut d, &reg, later, delta).unwrap();
    let (e, l) = (doc_clip(d.doc(), earlier), doc_clip(d.doc(), later));
    assert_eq!((e.start, e.end()), (0, 3 * PPQ + delta));
    assert_eq!((l.start, l.end()), (2 * PPQ + delta, 5 * PPQ));
    let zone = crossfades(&d.doc().song.tracks[0].lanes[0])[0];
    assert_eq!(
        (zone.start, zone.end),
        (2 * PPQ + delta, 3 * PPQ + delta),
        "the overlap keeps its length"
    );
    // Every frame outside the old and the new overlap plays what it played before.
    let moved = BEAT_F / 4;
    for frame in [
        0,
        BEAT_F,
        2 * BEAT_F - 1,
        3 * BEAT_F + moved + 1,
        4 * BEAT_F,
        5 * BEAT_F - 1,
    ] {
        let after = playing_at(&d, frame);
        assert_eq!(after, vec![(hash(1), frame)], "frame {frame}");
    }
    // One undo step puts the clips back.
    d.undo().unwrap();
    assert_eq!(*d.doc().song, *before);
    // The other way: the overlap moves earlier, the later clip starts on audio before its start.
    move_xfade(&mut d, &reg, later, -(PPQ / 2)).unwrap();
    let l = doc_clip(d.doc(), later);
    assert_eq!(l.start, 2 * PPQ - PPQ / 2);
    assert_eq!(audio_of(&d, later).source_offset, 2 * BEAT_F - BEAT_F / 2);
    assert_eq!(playing_at(&d, 4 * BEAT_F), vec![(hash(1), 4 * BEAT_F)]);
}

#[test]
fn a_crossfade_move_that_does_not_fit_is_refused_and_changes_nothing() {
    let (reg, mut d) = setup();
    let (earlier, later) = crossfade_pair(&mut d, &reg);
    let before = d.doc().song.clone();
    assert!(move_xfade(&mut d, &reg, later, 0).is_err());
    // The overlap would leave the later clip (it ends at beat 5; the zone ends at beat 3).
    assert!(move_xfade(&mut d, &reg, later, 2 * PPQ).is_err());
    // ... or the earlier one (it starts at tick 0).
    assert!(move_xfade(&mut d, &reg, later, -2 * PPQ).is_err());
    // A clip with nothing before it has no crossfade to move.
    assert!(move_xfade(&mut d, &reg, earlier, PPQ / 4).is_err());
    assert_eq!(*d.doc().song, *before);
}

#[test]
fn a_crossfade_move_needs_audio_in_the_file() {
    let (reg, mut d) = setup();
    // File 2 is four beats long: the earlier clip already plays up to its end.
    let earlier = put(
        &mut d,
        &reg,
        0,
        3 * PPQ,
        AudioClipContent {
            source_offset: BEAT_F,
            ..audio(2)
        },
    );
    let later = put(
        &mut d,
        &reg,
        2 * PPQ,
        2 * PPQ,
        AudioClipContent {
            source_offset: 0,
            ..audio(1)
        },
    );
    let _ = earlier;
    let err = move_xfade(&mut d, &reg, later, PPQ / 2).unwrap_err();
    assert!(err.to_string().contains("no audio"), "{err}");
    // The later clip starts at the top of its file: nothing lies before it either.
    let err = move_xfade(&mut d, &reg, later, -(PPQ / 2)).unwrap_err();
    assert!(err.to_string().contains("no audio"), "{err}");
}
