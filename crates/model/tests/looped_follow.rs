//! Looped audio takes (`SetTakeLoop`, the comp compiling one clip per repetition) and the
//! render of an audio loop into one clip (`RenderLoop`).

mod common;

use atomik_model::song::flatten_audio;
use atomik_model::takes::CompSegment;
use atomik_model::{
    AudioClipContent, Clip, ClipContent, ClipId, Command, Document, ModelError, SongCommand,
    TakeGroup, WorkflowCommand, load_project, save_project,
};
use common::audio::*;
use common::song::*;
use common::workflow::*;
use common::*;

/// A document with one audio track that holds one plain audio clip (asset 1) of `length` ticks
/// at tick 0, grouped as a one-take group.
fn one_take(reg: &atomik_device_sdk::Registry, length: i64) -> Document {
    let mut d = with_audio_track(reg);
    d.apply(add_audio_clip("audio-track", 0, length, 1), reg)
        .unwrap();
    let id = last(&d);
    d.apply(
        sc(SongCommand::GroupAsTakes {
            device: key("audio-track"),
            lane: 0,
            clips: vec![id],
            extra: vec![],
        }),
        reg,
    )
    .unwrap();
    d
}

fn group(d: &Document) -> TakeGroup {
    d.doc()
        .song
        .take_groups()
        .next()
        .expect("a group")
        .2
        .clone()
}

fn lane_clips(d: &Document) -> Vec<Clip> {
    let mut clips = d.doc().song.tracks[0].lanes[0].clips.clone();
    clips.sort_by_key(|c| (c.start, c.id));
    clips
}

fn loop_cmd(group: u32, take: usize, repeats: u32) -> Command {
    sc(SongCommand::SetTakeLoop {
        group,
        take,
        repeats,
    })
}

fn silence(start: i64, end: i64) -> Command {
    sc(SongCommand::SetCompSilence {
        group: 1,
        start,
        end,
    })
}

fn regions(d: &Document) -> Vec<atomik_model::song::AudioRegion> {
    let song = d.doc().song.clone();
    flatten_audio(song.track(&key("audio-track")).unwrap(), &song, 48_000)
}

#[test]
fn looping_an_audio_take_compiles_one_clip_per_repetition() {
    let reg = audio_registry();
    let mut d = one_take(&reg, BAR);
    one_step(&mut d, &reg, loop_cmd(1, 0, 3));
    let g = group(&d);
    assert_eq!(g.takes[0].length, 3 * BAR);
    assert_eq!(g.takes[0].loop_length, Some(BAR));
    // The comp ran to the end of the take and runs on to the new end.
    assert_eq!(g.comp, vec![CompSegment::new(0, 0, 3 * BAR)]);
    let clips = lane_clips(&d);
    assert_eq!(clips.len(), 3);
    for (k, c) in clips.iter().enumerate() {
        assert_eq!((c.start, c.length), (k as i64 * BAR, BAR));
        assert!(c.loop_length.is_none() && c.offset == 0);
    }
    let r = regions(&d);
    assert_eq!(r.len(), 3);
    assert!(r.iter().all(|r| r.source_offset == 0 && r.asset == hash(1)));
    // One repetition is the take again: no loop, one clip.
    one_step(&mut d, &reg, loop_cmd(1, 0, 1));
    let g = group(&d);
    assert_eq!((g.takes[0].length, g.takes[0].loop_length), (BAR, None));
    assert_eq!(lane_clips(&d).len(), 1);
}

#[test]
fn a_looped_audio_take_plays_what_the_same_looped_clip_played() {
    let reg = audio_registry();
    // The reference: a plain clip made to loop.
    let mut r = with_audio_track(&reg);
    r.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&r);
    r.apply(
        sc(SongCommand::SetClipGeometry {
            id,
            start: 0,
            length: 3 * BAR + BAR / 2,
            offset: 0,
            loop_length: Some(BAR),
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let mut d = one_take(&reg, BAR);
    // Loop the take 4 times, then silence the last half bar.
    d.apply(loop_cmd(1, 0, 4), &reg).unwrap();
    d.apply(silence(3 * BAR + BAR / 2, 4 * BAR), &reg).unwrap();
    let a = regions(&r);
    let b = regions(&d);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(
            (x.song_start_frame, x.source_offset, x.frames),
            (y.song_start_frame, y.source_offset, y.frames)
        );
    }
}

#[test]
fn a_cut_inside_a_repetition_fades_and_keeps_the_source_position() {
    let reg = audio_registry();
    let mut d = one_take(&reg, BAR);
    d.apply(loop_cmd(1, 0, 3), &reg).unwrap();
    // Only 1.5 bars to 2.5 bars play.
    d.apply(silence(0, BAR + BAR / 2), &reg).unwrap();
    d.apply(silence(2 * BAR + BAR / 2, 3 * BAR), &reg).unwrap();
    let clips = lane_clips(&d);
    assert_eq!(clips.len(), 2);
    let (ClipContent::Audio(first), ClipContent::Audio(second)) =
        (&clips[0].content, &clips[1].content)
    else {
        panic!("audio clips");
    };
    // The first piece is the second half of repetition 1: its source starts half way in.
    let song = &d.doc().song;
    let half = song.frame_clock(48_000).at(BAR / 2);
    assert_eq!((clips[0].start, clips[0].length), (BAR + BAR / 2, BAR / 2));
    assert_eq!(first.source_offset, half);
    assert!(first.fade_in > 0, "a cut inside a repetition fades in");
    assert_eq!(first.fade_out, 0, "the repetition end is a natural edge");
    // The second is the first half of repetition 2: from the start of the source.
    assert_eq!((clips[1].start, clips[1].length), (2 * BAR, BAR / 2));
    assert_eq!(second.source_offset, 0);
    assert_eq!(second.fade_in, 0);
    assert!(second.fade_out > 0);
}

#[test]
fn a_shorter_take_cuts_the_comp_and_the_loop_survives_the_file() {
    let reg = audio_registry();
    let mut d = one_take(&reg, BAR);
    d.apply(loop_cmd(1, 0, 4), &reg).unwrap();
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).expect("the file loads");
    assert_eq!(save_project(&loaded), text);
    let g = loaded.song.take_groups().next().unwrap().2.clone();
    assert_eq!(g.takes[0].loop_length, Some(BAR));
    d.apply(loop_cmd(1, 0, 2), &reg).unwrap();
    assert_eq!(group(&d).comp, vec![CompSegment::new(0, 0, 2 * BAR)]);
    assert_eq!(lane_clips(&d).len(), 2);
    // Undo walks back through both steps.
    d.undo().unwrap();
    assert_eq!(lane_clips(&d).len(), 4);
}

#[test]
fn a_loop_that_cannot_be_made_is_refused_and_changes_nothing() {
    let reg = audio_registry();
    let mut d = one_take(&reg, BAR);
    let e = rejected(&mut d, &reg, loop_cmd(1, 0, 0));
    assert!(matches!(e, ModelError::OutOfRange(_)));
    let e = rejected(&mut d, &reg, loop_cmd(1, 5, 2));
    assert!(matches!(e, ModelError::OutOfRange(_)));
    let e = rejected(&mut d, &reg, loop_cmd(9, 0, 2));
    assert!(matches!(e, ModelError::UnknownKey(_)));
    // The same count again changes nothing.
    let e = rejected(&mut d, &reg, loop_cmd(1, 0, 1));
    assert!(matches!(e, ModelError::BadSong(_)));
    let e = rejected(&mut d, &reg, loop_cmd(1, 0, u32::MAX));
    assert!(matches!(e, ModelError::OutOfRange(_)));
}

#[test]
fn a_reversed_audio_take_cannot_loop() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    d.apply(
        wf(WorkflowCommand::ReverseClips {
            ids: vec![id],
            graphical: false,
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::GroupAsTakes {
            device: key("audio-track"),
            lane: 0,
            clips: vec![id],
            extra: vec![],
        }),
        &reg,
    )
    .unwrap();
    let e = rejected(&mut d, &reg, loop_cmd(1, 0, 2));
    assert!(matches!(e, ModelError::BadSong(m) if m.contains("reversed")));
}

/// A looped audio clip `[0, 2 bars)` of asset 1 and a registered render (asset 2).
fn looped_clip(reg: &atomik_device_sdk::Registry) -> (Document, ClipId) {
    let mut d = with_audio_track(reg);
    d.apply(add_audio_clip("audio-track", 0, 2 * BAR, 1), reg)
        .unwrap();
    let id = last(&d);
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id,
            start: 0,
            length: 2 * BAR,
            offset: 0,
            loop_length: Some(BAR),
            muted: false,
            merge: None,
        }),
        reg,
    )
    .unwrap();
    (d, id)
}

fn render_cmd(id: ClipId, audio: AudioClipContent) -> Command {
    wf(WorkflowCommand::RenderLoop { id, audio })
}

#[test]
fn a_render_replaces_the_loop_with_one_plain_clip_in_one_step() {
    let reg = audio_registry();
    let (mut d, id) = looped_clip(&reg);
    d.apply(
        sc(SongCommand::SetClipMeta {
            ids: vec![id],
            name: atomik_model::MetaChange::Set("Loop".into()),
            color: atomik_model::MetaChange::Keep,
        }),
        &reg,
    )
    .unwrap();
    one_step(&mut d, &reg, render_cmd(id, audio(2)));
    let clips = lane_clips(&d);
    assert_eq!(clips.len(), 1);
    let c = &clips[0];
    assert_eq!((c.id, c.start, c.length), (id, 0, 2 * BAR));
    assert!(c.loop_length.is_none() && c.offset == 0);
    assert_eq!(c.name.as_deref(), Some("Loop"));
    match &c.content {
        ClipContent::Audio(a) => {
            assert_eq!((a.asset.as_str(), a.source_offset), (hash(2).as_str(), 0))
        }
        _ => panic!("audio"),
    }
    assert_eq!(d.undo_label(), Some("Convert Loop"));
    let r = regions(&d);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].asset, hash(2));
}

#[test]
fn a_render_is_refused_for_what_is_not_a_looped_audio_clip() {
    let reg = audio_registry();
    let (mut d, id) = looped_clip(&reg);
    // Audio that is not in the pool.
    let e = rejected(&mut d, &reg, render_cmd(id, audio(9)));
    assert!(matches!(e, ModelError::MissingAsset(_)));
    // A plain render only.
    let mut reversed = audio(2);
    reversed.reverse = true;
    let e = rejected(&mut d, &reg, render_cmd(id, reversed));
    assert!(matches!(e, ModelError::BadSong(_)));
    // An unknown clip.
    let e = rejected(&mut d, &reg, render_cmd(ClipId(99), audio(2)));
    assert!(matches!(
        e,
        ModelError::UnknownKey(_) | ModelError::BadSong(_)
    ));
    // A clip that does not loop.
    d.apply(add_audio_clip("audio-track", 4 * BAR, BAR, 1), &reg)
        .unwrap();
    let plain = last(&d);
    let e = rejected(&mut d, &reg, render_cmd(plain, audio(2)));
    assert!(matches!(e, ModelError::BadSong(m) if m.contains("looped")));
}
