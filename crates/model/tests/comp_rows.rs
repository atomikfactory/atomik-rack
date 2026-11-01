//! Comp rows: `DuplicateTake`, `MoveTake`, `DeleteUnusedTakes`, `SetCompSilence`, `SetTake` and
//! `SetCompXfade` (one undo step each), the crossfade a cut compiles to, and how the crossfade
//! is saved.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::flatten_audio;
use atomik_model::takes::CompSegment;
use atomik_model::{
    Clip, ClipContent, Command, Document, ModelError, SongCommand, TakeGroup, load_project,
    save_project,
};
use common::audio::*;
use common::song::*;
use common::*;

fn seg(take: usize, start: i64, end: i64) -> CompSegment {
    CompSegment::new(take, start, end)
}

fn take(start: i64, length: i64, key: u8) -> Clip {
    notes_clip(start, length, vec![note(0, PPQ / 4, key, 0.8)])
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

fn song_cmd(c: SongCommand) -> Command {
    sc(c)
}

/// Three note takes of two bars at keys 60, 61, 62; take 0 plays the first bar, take 2 the second.
fn three(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    d.apply(
        song_cmd(SongCommand::NewTakeGroup {
            device: key("lead"),
            lane: 0,
            takes: (0..3).map(|i| take(0, 2 * BAR, 60 + i)).collect(),
        }),
        reg,
    )
    .unwrap();
    d.apply(
        song_cmd(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: 0,
            end: BAR,
        }),
        reg,
    )
    .unwrap();
    d
}

/// The lane clips without ids, in order, to compare what plays across commands.
fn lane_shape(d: &Document) -> Vec<Clip> {
    let mut clips = d.doc().song.tracks[0].lanes[0].clips.clone();
    clips.sort_by_key(|c| (c.start, c.id));
    for c in &mut clips {
        c.id = atomik_model::ClipId(0);
    }
    clips
}

fn rejects(d: &mut Document, cmd: SongCommand, reg: &Registry) -> ModelError {
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    let e = d.apply(song_cmd(cmd), reg).expect_err("must be rejected");
    assert_eq!(save_project(d.doc()), before);
    assert_eq!(d.undo_depth(), depth);
    e
}

/// Applies `cmd` and checks it is one undo step that undoes and redoes exactly.
fn step(d: &mut Document, cmd: SongCommand, reg: &Registry) {
    let before = save_project(d.doc());
    let depth = d.undo_depth();
    d.apply(song_cmd(cmd), reg).unwrap();
    assert_eq!(d.undo_depth(), depth + 1);
    let after = save_project(d.doc());
    d.undo(reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
    d.redo(reg).unwrap();
    assert_eq!(save_project(d.doc()), after);
}

#[test]
fn a_duplicated_take_sits_above_its_source_and_the_comp_follows_the_takes() {
    let reg = registry();
    let mut d = three(&reg);
    let before = lane_shape(&d);
    step(
        &mut d,
        SongCommand::DuplicateTake { group: 1, take: 0 },
        &reg,
    );
    let g = group(&d);
    assert_eq!(g.takes.len(), 4);
    // The copy is take 1; the old takes 1 and 2 are now 2 and 3.
    assert_ne!(g.takes[0].id, g.takes[1].id);
    assert_eq!(g.takes[0].content, g.takes[1].content);
    assert_eq!(g.comp, [seg(0, 0, BAR), seg(3, BAR, 2 * BAR)]);
    assert_eq!(lane_shape(&d), before, "what plays does not change");
}

#[test]
fn a_take_moves_in_the_stack_without_changing_what_plays() {
    let reg = registry();
    let mut d = three(&reg);
    let before = lane_shape(&d);
    step(
        &mut d,
        SongCommand::MoveTake {
            group: 1,
            take: 0,
            to: 2,
        },
        &reg,
    );
    let g = group(&d);
    // Take 0 (key 60) is now the top take; the comp names it by its new index.
    assert_eq!(g.comp, [seg(2, 0, BAR), seg(1, BAR, 2 * BAR)]);
    assert_eq!(lane_shape(&d), before);
    // And back down again.
    d.apply(
        song_cmd(SongCommand::MoveTake {
            group: 1,
            take: 2,
            to: 0,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(group(&d).comp, [seg(0, 0, BAR), seg(2, BAR, 2 * BAR)]);
    assert_eq!(lane_shape(&d), before);
    let e = rejects(
        &mut d,
        SongCommand::MoveTake {
            group: 1,
            take: 1,
            to: 1,
        },
        &reg,
    );
    assert!(e.to_string().contains("already"), "{e}");
    rejects(
        &mut d,
        SongCommand::MoveTake {
            group: 1,
            take: 0,
            to: 3,
        },
        &reg,
    );
}

#[test]
fn deleting_the_unused_takes_keeps_the_comp_playing_the_same() {
    let reg = registry();
    let mut d = three(&reg);
    let before = lane_shape(&d);
    step(&mut d, SongCommand::DeleteUnusedTakes { group: 1 }, &reg);
    let g = group(&d);
    // Takes 0 and 2 are used; take 1 goes and take 2 becomes take 1.
    assert_eq!(g.takes.len(), 2);
    assert_eq!(g.comp, [seg(0, 0, BAR), seg(1, BAR, 2 * BAR)]);
    assert_eq!(lane_shape(&d), before);
    let e = rejects(&mut d, SongCommand::DeleteUnusedTakes { group: 1 }, &reg);
    assert!(e.to_string().contains("every take"), "{e}");
}

#[test]
fn an_all_silent_comp_keeps_its_takes() {
    let reg = registry();
    let mut d = three(&reg);
    d.apply(
        song_cmd(SongCommand::SetCompSilence {
            group: 1,
            start: 0,
            end: 2 * BAR,
        }),
        &reg,
    )
    .unwrap();
    assert!(group(&d).comp.is_empty());
    let e = rejects(&mut d, SongCommand::DeleteUnusedTakes { group: 1 }, &reg);
    assert!(e.to_string().contains("nothing to keep"), "{e}");
}

#[test]
fn a_silent_range_cuts_a_gap_out_of_the_comp() {
    let reg = registry();
    let mut d = three(&reg);
    step(
        &mut d,
        SongCommand::SetCompSilence {
            group: 1,
            start: BAR / 2,
            end: BAR + BAR / 2,
        },
        &reg,
    );
    assert_eq!(
        group(&d).comp,
        [seg(0, 0, BAR / 2), seg(2, BAR + BAR / 2, 2 * BAR)]
    );
    let clips = &d.doc().song.tracks[0].lanes[0].clips;
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].length, BAR / 2);
    // Empty, outside the group's range, and already silent are all refused.
    for (s, e) in [(5, 5), (-BAR, 10), (0, 3 * BAR), (BAR, BAR + 10)] {
        rejects(
            &mut d,
            SongCommand::SetCompSilence {
                group: 1,
                start: s,
                end: e,
            },
            &reg,
        );
    }
    // A segment can play again over the gap.
    d.apply(
        song_cmd(SongCommand::SetCompSegment {
            group: 1,
            take: 1,
            start: BAR / 2,
            end: BAR,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(group(&d).comp.len(), 3);
}

// ---- audio ------------------------------------------------------------------------------------

/// Two audio takes of four bars (assets 1 and 3, 480 000 frames each); take 1 plays the first and
/// last bar, take 0 the two bars between (cuts at `BAR` and at `3 * BAR`).
fn audio_group(reg: &Registry) -> Document {
    let mut d = with_audio_track(reg);
    d.apply(register(3, 480_000), reg).unwrap();
    let clip = |n: u8| Clip {
        content: ClipContent::Audio(audio(n)),
        ..notes_clip(0, 4 * BAR, Vec::new())
    };
    d.apply(
        song_cmd(SongCommand::NewTakeGroup {
            device: key("audio-track"),
            lane: 0,
            takes: vec![clip(1), clip(3)],
        }),
        reg,
    )
    .unwrap();
    d.apply(
        song_cmd(SongCommand::SetCompSegment {
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

fn xfade(at: i64, ticks: u64) -> SongCommand {
    SongCommand::SetCompXfade {
        group: 1,
        at,
        xfade: ticks,
    }
}

fn regions(d: &Document) -> Vec<atomik_model::song::AudioRegion> {
    flatten_audio(&d.doc().song.tracks[0], &d.doc().song, 48_000)
}

#[test]
fn a_cut_without_a_crossfade_compiles_the_same_whether_or_not_one_was_ever_set() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    let plain = lane_shape(&d);
    assert!(regions(&d).iter().all(|r| r.x_in == 0 && r.x_out == 0));
    d.apply(song_cmd(xfade(BAR, 480)), &reg).unwrap();
    assert_ne!(lane_shape(&d), plain);
    d.apply(song_cmd(xfade(BAR, 0)), &reg).unwrap();
    assert_eq!(lane_shape(&d), plain);
}

#[test]
fn a_crossfaded_cut_makes_two_overlapping_regions_whose_gains_sum_to_unity() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    // A quarter note: 24 000 frames at 120 BPM and 48 kHz.
    step(&mut d, xfade(BAR, PPQ as u64), &reg);
    assert_eq!(group(&d).comp[1].xfade, PPQ as u64);
    let r = regions(&d);
    let (a, b) = (
        r.iter().find(|r| r.x_out > 0).expect("a fading-out region"),
        r.iter().find(|r| r.x_in > 0).expect("a fading-in region"),
    );
    // Take 1 (asset 3) plays before the cut and runs on under take 0 (asset 1).
    assert_eq!(a.asset, hash(3));
    assert_eq!(b.asset, hash(1));
    assert_eq!(a.song_start_frame, b.song_start_frame);
    assert_eq!(a.frames, 24_000);
    assert_eq!(a.x_out, a.frames);
    assert_eq!(b.x_in, 24_000);
    // The earlier take keeps playing the same source frames across the cut.
    assert_eq!(a.source_offset, 96_000);
    for i in 0..a.frames {
        let (ga, gb) = (a.xfade_gain(i) as f64, b.xfade_gain(i) as f64);
        assert!(
            (ga * ga + gb * gb - 1.0).abs() < 1e-4,
            "frame {i}: {ga} {gb}"
        );
    }
    // No dip on top of the crossfade: the later clip has no fade-in at the cut.
    let lane = &d.doc().song.tracks[0].lanes[0];
    let later = lane.clips.iter().find(|c| c.start == BAR).unwrap();
    assert!(matches!(&later.content, ClipContent::Audio(c) if c.fade_in == 0));
    // The other cut, with no crossfade, still dips.
    let last = lane.clips.iter().find(|c| c.start == 3 * BAR).unwrap();
    assert!(matches!(&last.content, ClipContent::Audio(c) if c.fade_in > 0));
}

#[test]
fn a_crossfade_is_clamped_to_the_later_segment_and_to_what_the_earlier_take_has_left() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    // Longer than the segment after the cut (two bars): the overlap is that segment.
    d.apply(song_cmd(xfade(BAR, 10 * BAR as u64)), &reg)
        .unwrap();
    let r = regions(&d);
    let a = r.iter().find(|r| r.x_out > 0).expect("fading out");
    assert_eq!(a.frames, 2 * 96_000);
    // The earlier take ends a quarter note after the cut: that is all it has left to fade over.
    let mut d = with_audio_track(&reg);
    d.apply(register(3, 480_000), &reg).unwrap();
    let clip = |n: u8, length: i64| Clip {
        content: ClipContent::Audio(audio(n)),
        ..notes_clip(0, length, Vec::new())
    };
    d.apply(
        song_cmd(SongCommand::NewTakeGroup {
            device: key("audio-track"),
            lane: 0,
            takes: vec![clip(1, 4 * BAR), clip(3, 2 * BAR + PPQ)],
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        song_cmd(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: 2 * BAR,
            end: 4 * BAR,
        }),
        &reg,
    )
    .unwrap();
    d.apply(song_cmd(xfade(2 * BAR, BAR as u64)), &reg).unwrap();
    let r = regions(&d);
    let a = r.iter().find(|r| r.x_out > 0).expect("fading out");
    assert_eq!(a.frames, 24_000);
}

#[test]
fn a_crossfade_needs_a_cut_audio_takes_and_a_new_length() {
    let reg = registry();
    let mut d = three(&reg);
    let e = rejects(&mut d, xfade(BAR, 100), &reg);
    assert!(e.to_string().contains("audio"), "{e}");
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    let e = rejects(&mut d, xfade(BAR / 2, 100), &reg);
    assert!(e.to_string().contains("no segment"), "{e}");
    d.apply(song_cmd(xfade(BAR, 100)), &reg).unwrap();
    let e = rejects(&mut d, xfade(BAR, 100), &reg);
    assert!(e.to_string().contains("already"), "{e}");
}

#[test]
fn a_comp_edit_next_to_a_crossfaded_cut_resets_the_crossfade_it_cut_off() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    d.apply(song_cmd(xfade(BAR, 480)), &reg).unwrap();
    // Replacing the start of the crossfaded segment gives it a new start with no crossfade.
    d.apply(
        song_cmd(SongCommand::SetCompSegment {
            group: 1,
            take: 1,
            start: BAR,
            end: BAR + BAR / 2,
        }),
        &reg,
    )
    .unwrap();
    let g = group(&d);
    assert!(g.comp.iter().all(|s| s.xfade == 0), "{:?}", g.comp);
}

#[test]
fn a_take_has_a_level_and_an_offset_within_its_asset() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    step(
        &mut d,
        SongCommand::SetTake {
            group: 1,
            take: 0,
            gain_db: -6.0,
            source_offset_delta: 4_800,
        },
        &reg,
    );
    let g = group(&d);
    let ClipContent::Audio(a) = &g.takes[0].content else {
        panic!("audio")
    };
    assert_eq!((a.gain_db, a.source_offset), (-6.0, 4_800));
    assert_eq!(g.takes[0].start, 0, "the take keeps its start");
    // The compiled clips carry both: the first piece of take 0 starts a bar into the asset.
    let lane = &d.doc().song.tracks[0].lanes[0];
    let c = lane.clips.iter().find(|c| c.start == BAR).unwrap();
    assert!(matches!(&c.content, ClipContent::Audio(a)
        if (a.gain_db + 6.0).abs() < 1e-6 && a.source_offset == 96_000 + 4_800));
    // Back before the asset, past its end, out of the gain range, and a no-op.
    for (gain, delta) in [(-6.0, -5_000), (-6.0, 1_000_000), (500.0, 0), (-6.0, 0)] {
        rejects(
            &mut d,
            SongCommand::SetTake {
                group: 1,
                take: 0,
                gain_db: gain,
                source_offset_delta: delta,
            },
            &reg,
        );
    }
    // Exactly the first frame and the asset end are fine.
    let set = |delta| {
        song_cmd(SongCommand::SetTake {
            group: 1,
            take: 0,
            gain_db: -6.0,
            source_offset_delta: delta,
        })
    };
    d.apply(set(-4_800), &reg).unwrap();
    d.apply(set(480_000), &reg).unwrap();
    // A notes take has neither.
    let reg = registry();
    let mut d = three(&reg);
    let e = rejects(
        &mut d,
        SongCommand::SetTake {
            group: 1,
            take: 0,
            gain_db: 0.0,
            source_offset_delta: 1,
        },
        &reg,
    );
    assert!(e.to_string().contains("audio"), "{e}");
}

#[test]
fn reordering_and_duplicating_leave_the_flattened_audio_as_it_was() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    d.apply(song_cmd(xfade(BAR, PPQ as u64)), &reg).unwrap();
    let before = regions(&d);
    d.apply(
        song_cmd(SongCommand::DuplicateTake { group: 1, take: 0 }),
        &reg,
    )
    .unwrap();
    assert_eq!(regions(&d), before);
    d.apply(
        song_cmd(SongCommand::MoveTake {
            group: 1,
            take: 0,
            to: 2,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(regions(&d), before);
    d.apply(song_cmd(SongCommand::DeleteUnusedTakes { group: 1 }), &reg)
        .unwrap();
    assert_eq!(regions(&d), before);
}

#[test]
fn the_crossfade_is_saved_only_when_set_and_round_trips() {
    let reg = audio_registry();
    let mut d = audio_group(&reg);
    let plain = save_project(d.doc());
    assert!(
        plain.contains(&format!("[0, {BAR}, {}]", 3 * BAR)),
        "{plain}"
    );
    d.apply(song_cmd(xfade(BAR, PPQ as u64)), &reg).unwrap();
    let text = save_project(d.doc());
    assert!(
        text.contains(&format!("[0, {BAR}, {}, {PPQ}]", 3 * BAR)),
        "{text}"
    );
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(save_project(&loaded), text);
    let g = loaded.song.take_groups().next().unwrap().2.clone();
    assert_eq!(g.comp[1].xfade, PPQ as u64);
    assert_eq!(g.comp[0].xfade, 0);
    // A file written before the field existed (three-element segments) loads with none.
    let loaded = load_project(&plain, &reg).unwrap();
    assert!(
        loaded
            .song
            .take_groups()
            .all(|(_, _, g)| g.comp.iter().all(|s| s.xfade == 0))
    );
    // A segment with a fifth element is refused.
    let bad = text.replacen(&format!(", {PPQ}]"), &format!(", {PPQ}, 1]"), 1);
    assert!(load_project(&bad, &reg).is_err());
}
