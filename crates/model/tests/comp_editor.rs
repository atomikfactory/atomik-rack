//! What the audio clip editor's Comp mode applies: a drag on a take (`SetCompRange`, the range
//! and the crossfades at its cuts in one undo step) and Flatten Comp (`FlattenComp`, the render
//! of the comp replaces the group, one undo step).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::takes::CompSegment;
use atomik_model::{Clip, ClipContent, Command, Document, SongCommand, TakeGroup, save_project};
use common::audio::*;
use common::song::*;
use common::*;

fn cmd(c: SongCommand) -> Command {
    sc(c)
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

/// Two audio takes of four bars; take 0 plays the two bars in the middle, take 1 the rest.
fn audio_group(reg: &Registry) -> Document {
    let mut d = with_audio_track(reg);
    d.apply(register(3, 480_000), reg).unwrap();
    let clip = |n: u8| Clip {
        content: ClipContent::Audio(audio(n)),
        ..notes_clip(0, 4 * BAR, Vec::new())
    };
    d.apply(
        cmd(SongCommand::NewTakeGroup {
            device: key("audio-track"),
            lane: 0,
            takes: vec![clip(1), clip(3)],
        }),
        reg,
    )
    .unwrap();
    d.apply(
        cmd(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: BAR,
            end: 3 * BAR,
        }),
        reg,
    )
    .unwrap();
    d
}

fn range(take: usize, start: i64, end: i64, xfade: u64) -> SongCommand {
    SongCommand::SetCompRange {
        group: 1,
        take,
        start,
        end,
        xfade,
    }
}

#[test]
fn a_range_with_a_crossfade_is_one_undo_step() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    d.apply(cmd(range(1, BAR / 2, 2 * BAR, 480)), &reg).unwrap();
    assert_eq!(d.undo_depth(), depth + 1);
    let g = group(&d);
    // Take 1 grew over the first half of take 0's bars; the cut at 2 bars crossfades.
    let want = vec![
        CompSegment::new(1, 0, 2 * BAR),
        CompSegment {
            xfade: 480,
            ..CompSegment::new(0, 2 * BAR, 3 * BAR)
        },
        CompSegment::new(1, 3 * BAR, 4 * BAR),
    ];
    assert_eq!(g.comp, want);
    d.undo().unwrap();
    assert_eq!(save_project(d.doc()), before);
    d.redo().unwrap();
    assert_eq!(group(&d).comp, want);
}

#[test]
fn both_cuts_of_the_range_crossfade_when_another_take_meets_them() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    d.apply(cmd(range(0, BAR / 2, 2 * BAR, 240)), &reg).unwrap();
    let g = group(&d);
    // Take 0 now plays from half a bar: the cut at half a bar (take 1 ends there) crossfades.
    assert_eq!(g.comp[1].start, BAR / 2);
    assert_eq!(g.comp[1].xfade, 240);
    assert_eq!(g.comp[0].xfade, 0);
    // The compiled clips are the ones a SetCompSegment would give, plus the overlap.
    assert!(g.clips.len() >= 3);
}

#[test]
fn a_range_in_a_note_group_ignores_the_crossfade_and_a_bad_range_is_refused() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    let take = |key: u8| notes_clip(0, 2 * BAR, vec![note(0, PPQ / 4, key, 0.8)]);
    d.apply(
        cmd(SongCommand::NewTakeGroup {
            device: key("lead"),
            lane: 0,
            takes: vec![take(60), take(61)],
        }),
        &reg,
    )
    .unwrap();
    d.apply(cmd(range(0, 0, BAR, 480)), &reg).unwrap();
    assert!(group(&d).comp.iter().all(|s| s.xfade == 0));
    let before = save_project(d.doc());
    assert!(d.apply(cmd(range(0, BAR, BAR, 0)), &reg).is_err());
    assert!(d.apply(cmd(range(5, 0, BAR, 0)), &reg).is_err());
    assert_eq!(save_project(d.doc()), before);
}

#[test]
fn flatten_comp_puts_the_render_in_place_of_the_group_in_one_step() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    d.apply(register(5, 4 * 96_000), &reg).unwrap();
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    d.apply(
        cmd(SongCommand::FlattenComp {
            group: 1,
            audio: audio(5),
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(d.undo_depth(), depth + 1);
    assert!(d.doc().song.take_groups().next().is_none());
    let clips = &d.doc().song.tracks[0].lanes[0].clips;
    assert_eq!(clips.len(), 1, "the compiled clips are gone");
    let c = &clips[0];
    // The comp plays from the first bar's start to the end of take 1's last segment.
    assert_eq!((c.start, c.length), (0, 4 * BAR));
    let ClipContent::Audio(a) = &c.content else {
        panic!("an audio clip")
    };
    assert_eq!(a.asset, hash(5));
    let after = save_project(d.doc());
    d.undo().unwrap();
    assert_eq!(save_project(d.doc()), before);
    assert_eq!(group(&d).comp.len(), 3);
    d.redo().unwrap();
    assert_eq!(save_project(d.doc()), after);
}

#[test]
fn flatten_comp_refuses_what_it_cannot_render() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    d.apply(register(5, 96_000), &reg).unwrap();
    let before = save_project(d.doc());
    let flatten = |audio| cmd(SongCommand::FlattenComp { group: 1, audio });
    // Not registered.
    assert!(d.apply(flatten(audio(9)), &reg).is_err());
    // Reversed.
    let mut a = audio(5);
    a.reverse = true;
    assert!(d.apply(flatten(a), &reg).is_err());
    // Warped.
    let mut a = audio(5);
    a.warp.enabled = true;
    assert!(d.apply(flatten(a), &reg).is_err());
    assert!(
        d.apply(
            cmd(SongCommand::FlattenComp {
                group: 7,
                audio: audio(5)
            }),
            &reg
        )
        .is_err()
    );
    assert_eq!(save_project(d.doc()), before);
}
