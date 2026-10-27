//! The song End: the accessor, the command, and the end moving out by itself when a clip is
//! drawn, moved or stretched past it (in the same undo step as the clip edit).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{ClipContent, Document, SongCommand, load_project, save_project};
use common::song::*;
use common::*;

/// The song with one track on `lead`; the End is at bar 5.
fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    d.apply(sc(SongCommand::SetSongEnd(4 * BAR)), reg).unwrap();
    d
}

fn add(d: &mut Document, reg: &Registry, start: i64, length: i64) {
    let c = clip(start, length, ClipContent::Notes(Vec::new()));
    d.apply(add_clip("lead", 0, c), reg).unwrap();
}

#[test]
fn end_tick_is_the_song_end() {
    let reg = registry();
    let d = doc(&reg);
    assert_eq!(d.doc().song.end_tick(), 4 * BAR);
    assert_eq!(d.doc().song.end_tick(), d.doc().song.end);
}

#[test]
fn a_clip_inside_the_end_leaves_it_alone() {
    let reg = registry();
    let mut d = doc(&reg);
    add(&mut d, &reg, 3 * BAR, BAR);
    assert_eq!(d.doc().song.end_tick(), 4 * BAR);
}

#[test]
fn a_clip_past_the_end_moves_it_to_the_next_bar_line_and_undo_restores_both() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    add(&mut d, &reg, 4 * BAR, BAR + PPQ);
    assert_eq!(
        d.doc().song.end_tick(),
        6 * BAR,
        "bar line at or after the clip's end"
    );
    assert_eq!(clip_ids(d.doc()).len(), 1);
    d.undo(&reg).unwrap();
    assert_eq!(
        d.doc(),
        &before,
        "one undo step takes the clip and the End back"
    );
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().song.end_tick(), 6 * BAR);
}

#[test]
fn a_clip_ending_on_a_bar_line_past_the_end_sets_the_end_there() {
    let reg = registry();
    let mut d = doc(&reg);
    add(&mut d, &reg, 4 * BAR, BAR);
    assert_eq!(d.doc().song.end_tick(), 5 * BAR);
}

#[test]
fn a_drag_past_the_end_is_one_undo_step_and_the_end_never_moves_back() {
    let reg = registry();
    let mut d = doc(&reg);
    add(&mut d, &reg, 0, BAR);
    let id = clip_ids(d.doc())[0];
    let before = d.doc().clone();
    d.end_merge();
    for start in [3 * BAR, 5 * BAR, 7 * BAR, 2 * BAR] {
        d.apply(
            sc(SongCommand::SetClipGeometry {
                id,
                start,
                length: BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                merge: Some(9),
            }),
            &reg,
        )
        .unwrap();
    }
    d.end_merge();
    assert_eq!(
        d.doc().song.end_tick(),
        8 * BAR,
        "the farthest the drag reached"
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn set_song_end_still_sets_exactly_what_it_is_given_even_inside_the_clips() {
    let reg = registry();
    let mut d = doc(&reg);
    add(&mut d, &reg, 4 * BAR, BAR);
    d.apply(sc(SongCommand::SetSongEnd(2 * BAR + 5)), &reg)
        .unwrap();
    assert_eq!(d.doc().song.end_tick(), 2 * BAR + 5);
}

#[test]
fn the_end_round_trips_through_the_project_file() {
    let reg = registry();
    let mut d = doc(&reg);
    add(&mut d, &reg, 4 * BAR, BAR + 1);
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song.end_tick(), 6 * BAR);
    assert_eq!(save_project(&loaded), text);
}
