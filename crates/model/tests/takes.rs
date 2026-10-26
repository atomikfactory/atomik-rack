//! Takes and comping: `NewTakeGroup`, `PickTake`, `SetCompSegment`, `DeleteTake` and
//! `FlattenTakes` (one undo step each, compiled to plain clips), what the engine's flatten sees,
//! the audio fades at comp cuts, relinking, and project format 20.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::{flatten_audio, flatten_notes};
use atomik_model::takes::{COMP_FADE_FRAMES, CompSegment};
use atomik_model::{
    ClipContent, ClipId, Command, Document, FadeCurve, ModelError, PROJECT_VERSION, SongCommand,
    SyncOp, load_project, save_project,
};
use common::audio::*;
use common::song::*;
use common::*;
use proptest::prelude::*;
use serde_json::Value;

/// A note take of `length` ticks from `start` with one note: key `key`, at its start.
fn take(start: i64, length: i64, key: u8) -> atomik_model::Clip {
    notes_clip(start, length, vec![note(0, PPQ / 4, key, 0.8)])
}

fn new_group(d: &mut Document, reg: &Registry, takes: Vec<atomik_model::Clip>) {
    d.apply(
        sc(SongCommand::NewTakeGroup {
            device: key("lead"),
            lane: 0,
            takes,
        }),
        reg,
    )
    .unwrap();
}

/// Four takes of two bars at keys 60, 61, 62, 63, as a loop recording of four laps makes.
fn four_takes(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    new_group(
        &mut d,
        reg,
        (0..4).map(|i| take(0, 2 * BAR, 60 + i as u8)).collect(),
    );
    d
}

fn group(d: &Document) -> atomik_model::TakeGroup {
    d.doc()
        .song
        .take_groups()
        .next()
        .expect("a group")
        .2
        .clone()
}

/// The keys the lead track plays, in time order.
fn played(d: &Document) -> Vec<(i64, u8)> {
    flatten_notes(&d.doc().song.tracks[0])
        .into_iter()
        .map(|(t, _, k, _)| (t, k))
        .collect()
}

fn lane_clip_count(d: &Document) -> usize {
    d.doc().song.tracks[0].lanes[0].clips.len()
}

fn rejects(d: &mut Document, cmd: Command, reg: &Registry) -> ModelError {
    let before = save_project(d.doc());
    let undo = d.can_undo();
    let e = d.apply(cmd, reg).expect_err("command must be rejected");
    assert_eq!(
        save_project(d.doc()),
        before,
        "a rejected command changes nothing"
    );
    assert_eq!(d.can_undo(), undo);
    e
}

fn seg(take: usize, start: i64, end: i64) -> CompSegment {
    CompSegment { take, start, end }
}

// ---- NewTakeGroup -----------------------------------------------------------------------------

#[test]
fn a_new_group_plays_the_last_take_and_keeps_the_others_off_the_lane() {
    let reg = registry();
    let d = four_takes(&reg);
    let g = group(&d);
    assert_eq!(g.takes.len(), 4);
    assert_eq!(g.comp, [seg(3, 0, 2 * BAR)]);
    assert_eq!(g.id, 1);
    assert_eq!(g.picked(), Some(3));
    // Only the compiled clip is on the lane; the takes are not.
    assert_eq!(lane_clip_count(&d), 1);
    assert_eq!(g.clips.len(), 1);
    assert_eq!(played(&d), [(0, 63)]);
    // Every clip id is unique: four takes, then the compiled clip.
    let ids: Vec<u64> = g
        .takes
        .iter()
        .map(|c| c.id.0)
        .chain(g.clips.iter().map(|c| c.0))
        .collect();
    assert_eq!(ids, [1, 2, 3, 4, 5]);
    assert_eq!(d.doc().song.next_clip_id, 6);
}

#[test]
fn a_new_group_is_one_undo_step_and_redo_restores_it_exactly() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    let before = save_project(d.doc());
    new_group(&mut d, &reg, vec![take(0, BAR, 60), take(0, BAR, 61)]);
    let after = save_project(d.doc());
    assert_ne!(before, after);
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
    assert_eq!(d.doc().song.take_groups().count(), 0);
    d.redo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), after);
}

#[test]
fn a_shorter_last_take_hides_the_earlier_take_only_where_it_has_content() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    new_group(&mut d, &reg, vec![take(0, 2 * BAR, 60), take(0, BAR, 61)]);
    assert_eq!(group(&d).comp, [seg(1, 0, BAR), seg(0, BAR, 2 * BAR)]);
    // The second bar is the first take cut at the bar: its note sits at the clip start (tick 0)
    // of the take, so only the first bar's note plays, from the last take.
    assert_eq!(played(&d), [(0, 61)]);
    assert_eq!(lane_clip_count(&d), 2);
}

#[test]
fn new_group_ops_report_a_song_change() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    let ops = d
        .apply(
            sc(SongCommand::NewTakeGroup {
                device: key("lead"),
                lane: 0,
                takes: vec![take(0, BAR, 60)],
            }),
            &reg,
        )
        .unwrap();
    assert!(ops.contains(&SyncOp::SongChanged));
}

#[test]
fn a_new_group_is_refused_for_bad_input_and_changes_nothing() {
    let reg = audio_registry_no_console();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    let cmd = |lane, takes| {
        sc(SongCommand::NewTakeGroup {
            device: key("lead"),
            lane,
            takes,
        })
    };
    assert!(
        rejects(&mut d, cmd(0, vec![]), &reg)
            .to_string()
            .contains("1..=")
    );
    assert!(
        rejects(&mut d, cmd(3, vec![take(0, BAR, 60)]), &reg)
            .to_string()
            .contains("lane")
    );
    // A bad note is caught like AddClip catches it.
    let bad = notes_clip(0, BAR, vec![note(0, 10, 200, 0.5)]);
    rejects(&mut d, cmd(0, vec![take(0, BAR, 60), bad]), &reg);
    // An audio clip does not fit a notes lane.
    let audio_clip = atomik_model::Clip {
        content: ClipContent::Audio(audio(1)),
        ..take(0, BAR, 60)
    };
    rejects(&mut d, cmd(0, vec![audio_clip]), &reg);
    // No track.
    rejects(
        &mut d,
        sc(SongCommand::NewTakeGroup {
            device: key("nobody"),
            lane: 0,
            takes: vec![take(0, BAR, 60)],
        }),
        &reg,
    );
    // An automation lane takes no takes.
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("lead"),
            param: "filter.cutoff".into(),
        }),
        &reg,
    )
    .unwrap();
    let e = rejects(&mut d, cmd(1, vec![take(0, BAR, 60)]), &reg);
    assert!(e.to_string().contains("notes and audio"), "{e}");
}

// ---- PickTake ---------------------------------------------------------------------------------

#[test]
fn picking_a_take_plays_it_alone_over_the_whole_range() {
    let reg = registry();
    let mut d = four_takes(&reg);
    d.apply(sc(SongCommand::PickTake { group: 1, take: 1 }), &reg)
        .unwrap();
    assert_eq!(group(&d).comp, [seg(1, 0, 2 * BAR)]);
    assert_eq!(played(&d), [(0, 61)]);
    assert_eq!(lane_clip_count(&d), 1);
    // The group's id list follows the swap, and the old compiled clip is gone.
    assert_eq!(group(&d).clips.len(), 1);
    assert!(d.doc().song.clip(group(&d).clips[0]).is_some());
    assert!(d.doc().song.clip(ClipId(5)).is_none());
}

#[test]
fn a_pick_is_one_undo_step_and_undo_brings_the_old_comp_back() {
    let reg = registry();
    let mut d = four_takes(&reg);
    let before = save_project(d.doc());
    d.apply(sc(SongCommand::PickTake { group: 1, take: 0 }), &reg)
        .unwrap();
    d.apply(sc(SongCommand::PickTake { group: 1, take: 2 }), &reg)
        .unwrap();
    assert_eq!(played(&d), [(0, 62)]);
    d.undo(&reg).unwrap();
    assert_eq!(played(&d), [(0, 60)]);
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
    assert_eq!(played(&d), [(0, 63)]);
}

#[test]
fn picking_refuses_an_unknown_group_or_take() {
    let reg = registry();
    let mut d = four_takes(&reg);
    rejects(
        &mut d,
        sc(SongCommand::PickTake { group: 9, take: 0 }),
        &reg,
    );
    let e = rejects(
        &mut d,
        sc(SongCommand::PickTake { group: 1, take: 4 }),
        &reg,
    );
    assert!(e.to_string().contains("4 takes"), "{e}");
}

// ---- SetCompSegment ---------------------------------------------------------------------------

fn comp(d: &mut Document, reg: &Registry, take: usize, start: i64, end: i64) {
    d.apply(
        sc(SongCommand::SetCompSegment {
            group: 1,
            take,
            start,
            end,
        }),
        reg,
    )
    .unwrap();
}

#[test]
fn a_comp_segment_plays_that_part_of_another_take() {
    let reg = registry();
    let mut d = four_takes(&reg);
    comp(&mut d, &reg, 0, BAR / 2, BAR);
    assert_eq!(
        group(&d).comp,
        [
            seg(3, 0, BAR / 2),
            seg(0, BAR / 2, BAR),
            seg(3, BAR, 2 * BAR)
        ]
    );
    // Three plain clips on the lane. A take's note sits at the start of the take, so the
    // note of take 0 is outside its segment (it starts at tick 0): only the others play.
    assert_eq!(lane_clip_count(&d), 3);
    assert_eq!(played(&d), [(0, 63)]);
}

#[test]
fn a_comp_segment_carries_the_notes_of_its_take_at_the_right_time() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    let ev = |key: u8| {
        notes_clip(
            0,
            2 * BAR,
            vec![
                note(0, PPQ, key, 0.8),
                note(BAR, PPQ, key + 12, 0.8),
                note(BAR + PPQ, PPQ, key + 24, 0.8),
            ],
        )
    };
    new_group(&mut d, &reg, vec![ev(30), ev(40)]);
    // Bar 2 from the first take, the rest from the second.
    comp(&mut d, &reg, 0, BAR, 2 * BAR);
    assert_eq!(played(&d), [(0, 40), (BAR, 42), (BAR + PPQ, 54)]);
}

#[test]
fn a_comp_edit_is_one_undo_step() {
    let reg = registry();
    let mut d = four_takes(&reg);
    let before = save_project(d.doc());
    comp(&mut d, &reg, 1, 0, BAR);
    comp(&mut d, &reg, 2, BAR, 2 * BAR);
    assert_eq!(group(&d).comp, [seg(1, 0, BAR), seg(2, BAR, 2 * BAR)]);
    d.undo(&reg).unwrap();
    assert_eq!(group(&d).comp, [seg(1, 0, BAR), seg(3, BAR, 2 * BAR)]);
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
    d.redo(&reg).unwrap();
    d.redo(&reg).unwrap();
    assert_eq!(group(&d).comp, [seg(1, 0, BAR), seg(2, BAR, 2 * BAR)]);
}

#[test]
fn a_comp_segment_must_lie_in_the_group_range_and_name_a_take() {
    let reg = registry();
    let mut d = four_takes(&reg);
    let cmd = |take, start, end| {
        sc(SongCommand::SetCompSegment {
            group: 1,
            take,
            start,
            end,
        })
    };
    assert!(
        rejects(&mut d, cmd(0, 10, 10), &reg)
            .to_string()
            .contains("empty")
    );
    assert!(
        rejects(&mut d, cmd(0, 20, 10), &reg)
            .to_string()
            .contains("empty")
    );
    assert!(
        rejects(&mut d, cmd(0, -5, 10), &reg)
            .to_string()
            .contains("outside")
    );
    assert!(
        rejects(&mut d, cmd(0, 0, 3 * BAR), &reg)
            .to_string()
            .contains("outside")
    );
    assert!(
        rejects(&mut d, cmd(7, 0, 10), &reg)
            .to_string()
            .contains("names take 7")
    );
    rejects(
        &mut d,
        sc(SongCommand::SetCompSegment {
            group: 2,
            take: 0,
            start: 0,
            end: 10,
        }),
        &reg,
    );
}

// ---- DeleteTake and FlattenTakes --------------------------------------------------------------

#[test]
fn deleting_a_take_hands_its_ranges_to_the_take_below_and_renumbers() {
    let reg = registry();
    let mut d = four_takes(&reg);
    comp(&mut d, &reg, 1, 0, BAR);
    // comp: take 1 for bar 1, take 3 for bar 2.
    d.apply(sc(SongCommand::DeleteTake { group: 1, take: 3 }), &reg)
        .unwrap();
    let g = group(&d);
    assert_eq!(g.takes.len(), 3);
    assert_eq!(g.comp, [seg(1, 0, BAR), seg(2, BAR, 2 * BAR)]);
    assert_eq!(played(&d), [(0, 61)]);
    // Deleting take 0 renumbers the rest.
    d.apply(sc(SongCommand::DeleteTake { group: 1, take: 0 }), &reg)
        .unwrap();
    assert_eq!(group(&d).comp, [seg(0, 0, BAR), seg(1, BAR, 2 * BAR)]);
    assert_eq!(group(&d).takes.len(), 2);
}

#[test]
fn deleting_the_last_take_removes_the_group_and_its_clips_and_undo_brings_both_back() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    new_group(&mut d, &reg, vec![take(0, BAR, 60)]);
    let before = save_project(d.doc());
    d.apply(sc(SongCommand::DeleteTake { group: 1, take: 0 }), &reg)
        .unwrap();
    assert_eq!(d.doc().song.take_groups().count(), 0);
    assert_eq!(lane_clip_count(&d), 0);
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
}

#[test]
fn deleting_a_take_that_shrinks_the_range_cuts_the_comp_back() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    new_group(&mut d, &reg, vec![take(0, BAR, 60), take(0, 2 * BAR, 61)]);
    assert_eq!(group(&d).range(), (0, 2 * BAR));
    d.apply(sc(SongCommand::DeleteTake { group: 1, take: 1 }), &reg)
        .unwrap();
    assert_eq!(group(&d).range(), (0, BAR));
    assert_eq!(group(&d).comp, [seg(0, 0, BAR)]);
}

#[test]
fn flattening_keeps_the_comp_clips_and_drops_the_takes() {
    let reg = registry();
    let mut d = four_takes(&reg);
    comp(&mut d, &reg, 0, BAR / 2, BAR);
    let clips_before = d.doc().song.tracks[0].lanes[0].clips.clone();
    let played_before = played(&d);
    let ops = d
        .apply(sc(SongCommand::FlattenTakes { group: 1 }), &reg)
        .unwrap();
    assert!(
        !ops.contains(&SyncOp::SongChanged),
        "the sound does not change"
    );
    assert_eq!(d.doc().song.take_groups().count(), 0);
    assert_eq!(d.doc().song.tracks[0].lanes[0].clips, clips_before);
    assert_eq!(played(&d), played_before);
    d.undo(&reg).unwrap();
    assert_eq!(group(&d).takes.len(), 4);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().song.take_groups().count(), 0);
}

#[test]
fn group_ids_grow_and_a_removed_track_takes_its_groups_along() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    new_group(&mut d, &reg, vec![take(0, BAR, 60)]);
    new_group(&mut d, &reg, vec![take(4 * BAR, BAR, 61)]);
    let ids: Vec<u32> = d.doc().song.take_groups().map(|(_, _, g)| g.id).collect();
    assert_eq!(ids, [1, 2]);
    let with = save_project(d.doc());
    d.apply(
        sc(SongCommand::RemoveTrack {
            device: key("lead"),
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().song.take_groups().count(), 0);
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), with);
}

// ---- audio ------------------------------------------------------------------------------------

fn audio_doc(reg: &Registry) -> Document {
    let mut d = with_audio_track(reg);
    d.apply(register(3, 480_000), reg).unwrap();
    let clip = |n: u8, offset: u64| atomik_model::Clip {
        content: ClipContent::Audio(atomik_model::AudioClipContent {
            source_offset: offset,
            ..audio(n)
        }),
        ..clip_base(0, 4 * BAR)
    };
    d.apply(
        sc(SongCommand::NewTakeGroup {
            device: key("audio-track"),
            lane: 0,
            takes: vec![clip(1, 0), clip(3, 0)],
        }),
        reg,
    )
    .unwrap();
    d
}

fn clip_base(start: i64, length: i64) -> atomik_model::Clip {
    notes_clip(start, length, Vec::new())
}

#[test]
fn audio_takes_compile_to_audio_clips_with_source_offsets_and_cut_fades() {
    let reg = audio_registry();
    let mut d = audio_doc(&reg);
    d.apply(
        sc(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: BAR,
            end: 2 * BAR,
        }),
        &reg,
    )
    .unwrap();
    let lane = &d.doc().song.tracks[0].lanes[0];
    assert_eq!(lane.clips.len(), 3);
    let a: Vec<_> = lane
        .clips
        .iter()
        .map(|c| match &c.content {
            ClipContent::Audio(a) => (
                c.start,
                c.length,
                a.asset.clone(),
                a.source_offset,
                a.fade_in,
                a.fade_out,
            ),
            ClipContent::Notes(_) | ClipContent::Automation(_) => panic!("audio"),
        })
        .collect();
    // 120 BPM, 48 kHz: a bar is 96 000 frames.
    assert_eq!(a[0], (0, BAR, hash(3), 0, 0, COMP_FADE_FRAMES));
    assert_eq!(
        a[1],
        (
            BAR,
            BAR,
            hash(1),
            96_000,
            COMP_FADE_FRAMES,
            COMP_FADE_FRAMES
        )
    );
    assert_eq!(
        a[2],
        (2 * BAR, 2 * BAR, hash(3), 192_000, COMP_FADE_FRAMES, 0)
    );
    assert!(lane.clips.iter().all(|c| matches!(&c.content, ClipContent::Audio(a) if a.fade_curve == FadeCurve::EqualPower || (a.fade_in == 0 && a.fade_out == 0))));
}

#[test]
fn the_flattened_audio_regions_follow_the_comp_without_gaps() {
    let reg = audio_registry();
    let mut d = audio_doc(&reg);
    d.apply(
        sc(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: BAR,
            end: 2 * BAR,
        }),
        &reg,
    )
    .unwrap();
    let regions = flatten_audio(&d.doc().song.tracks[0], &d.doc().song, 48_000);
    let spans: Vec<(String, u64, u64)> = regions
        .iter()
        .map(|r| (r.asset.clone(), r.song_start_frame, r.frames))
        .collect();
    assert_eq!(
        spans,
        [
            (hash(3), 0, 96_000),
            (hash(1), 96_000, 96_000),
            (hash(3), 192_000, 192_000)
        ]
    );
    // Neighbouring regions touch: no frame is lost or doubled.
    for w in regions.windows(2) {
        assert_eq!(w[0].song_start_frame + w[0].frames, w[1].song_start_frame);
    }
}

#[test]
fn audio_takes_count_as_referenced_assets_and_relink_swaps_them_in_one_step() {
    let reg = audio_registry();
    let mut d = audio_doc(&reg);
    // Take 0 (asset 1) is hidden under the comp but its asset stays in the document.
    assert!(d.doc().referenced_assets().contains(&hash(1)));
    d.apply(register(4, 480_000), &reg).unwrap();
    let before = save_project(d.doc());
    d.apply(
        Command::RelinkAsset {
            from: hash(1),
            to: hash(4),
        },
        &reg,
    )
    .unwrap();
    let g = group(&d);
    assert!(matches!(&g.takes[0].content, ClipContent::Audio(a) if a.asset == hash(4)));
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before);
}

#[test]
fn an_audio_take_on_a_notes_lane_and_a_notes_take_on_an_audio_lane_are_refused() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    let e = d
        .apply(
            sc(SongCommand::NewTakeGroup {
                device: key("audio-track"),
                lane: 0,
                takes: vec![take(0, BAR, 60)],
            }),
            &reg,
        )
        .unwrap_err();
    assert!(!e.to_string().is_empty());
    assert_eq!(d.doc().song.take_groups().count(), 0);
}

// ---- project format 20 ------------------------------------------------------------------------

fn mixed_doc(reg: &Registry) -> Document {
    let mut d = four_takes(reg);
    comp(&mut d, reg, 1, BAR / 2, BAR);
    comp(&mut d, reg, 0, 3 * BAR / 2, 2 * BAR);
    d
}

#[test]
fn take_groups_round_trip_byte_for_byte_and_the_version_is_twenty_two() {
    let reg = registry();
    let d = mixed_doc(&reg);
    let text = save_project(d.doc());
    assert_eq!(PROJECT_VERSION, 31);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 31);
    assert!(text.contains("\"take_groups\""));
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(save_project(&loaded), text);
    assert_eq!(loaded.song.take_groups().count(), 1);
    let (_, lane, g) = loaded.song.take_groups().next().unwrap();
    assert_eq!(lane, 0);
    assert_eq!(g.id, 1);
    assert_eq!(g.comp, group(&d).comp);
    assert_eq!(g.takes, group(&d).takes);
    assert_eq!(g.clips, group(&d).clips);
}

#[test]
fn a_lane_without_groups_writes_no_field() {
    let reg = registry();
    let d = song_start(&reg);
    assert!(!save_project(d.doc()).contains("take_groups"));
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(add_clip("lead", 0, take(0, BAR, 60)), &reg)
        .unwrap();
    assert!(!save_project(d.doc()).contains("take_groups"));
}

#[test]
fn a_version_19_file_loads_without_groups_and_refuses_the_field() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(add_clip("lead", 0, take(0, BAR, 60)), &reg)
        .unwrap();
    let text = save_project(d.doc());
    let v19 = text.replace("\"version\": 31", "\"version\": 19");
    let loaded = load_project(&v19, &reg).unwrap();
    assert_eq!(loaded.song.take_groups().count(), 0);
    assert!(save_project(&loaded).contains("\"version\": 31"));
    // The field is unknown before version 20.
    let with = save_project(mixed_doc(&reg).doc()).replace("\"version\": 31", "\"version\": 19");
    let e = load_project(&with, &reg).unwrap_err().to_string();
    assert!(e.contains("take_groups"), "{e}");
}

fn edited(text: &str, f: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    f(&mut v);
    v.to_string()
}

fn first_group(v: &mut Value) -> &mut Value {
    &mut v["song"]["tracks"][0]["lanes"][0]["take_groups"][0]
}

#[test]
fn a_bad_group_is_refused_with_its_json_path() {
    let reg = registry();
    let text = save_project(mixed_doc(&reg).doc());
    let load = |f: &dyn Fn(&mut Value)| {
        load_project(&edited(&text, |v| f(v)), &reg)
            .unwrap_err()
            .to_string()
    };
    let e = load(&|v| first_group(v)["takes"] = serde_json::json!([]));
    assert!(e.contains("take_groups[0].takes"), "{e}");
    let e = load(&|v| first_group(v)["comp"] = serde_json::json!([[9, 0, 10]]));
    assert!(
        e.contains("take_groups[0].comp") && e.contains("names take 9"),
        "{e}"
    );
    let e = load(&|v| first_group(v)["comp"] = serde_json::json!([[0, 5, 5]]));
    assert!(e.contains("empty"), "{e}");
    let e = load(&|v| first_group(v)["comp"] = serde_json::json!([[0, 0, 99999999999i64]]));
    assert!(e.contains("take_groups[0].comp"), "{e}");
    let e = load(&|v| first_group(v)["clips"] = serde_json::json!([9999]));
    assert!(
        e.contains("take_groups[0].clips[0]") && e.contains("not on this lane"),
        "{e}"
    );
    let e = load(&|v| first_group(v)["clips"] = serde_json::json!([1]));
    assert!(
        e.contains("not on this lane"),
        "take clip ids are not lane clips: {e}"
    );
    let e = load(&|v| first_group(v)["extra"] = serde_json::json!(1));
    assert!(e.contains("extra"), "{e}");
    // A take reusing the id of a lane clip is a duplicate id.
    let e = load(&|v| {
        let lane_id = v["song"]["tracks"][0]["lanes"][0]["clips"][0]["id"].clone();
        first_group(v)["takes"][0]["id"] = lane_id;
    });
    assert!(e.contains("duplicate clip id"), "{e}");
}

#[test]
fn a_group_may_name_one_lane_clip_only_once() {
    let reg = registry();
    let text = save_project(mixed_doc(&reg).doc());
    let e = load_project(
        &edited(&text, |v| {
            let id = v["song"]["tracks"][0]["lanes"][0]["clips"][0]["id"].clone();
            first_group(v)["clips"] = serde_json::json!([id, id]);
        }),
        &reg,
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("already a comp clip"), "{e}");
}

// ---- random edits -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Pick(usize),
    Comp(usize, i64, i64),
    Delete(usize),
    Flatten,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0usize..5).prop_map(Op::Pick),
        6 => (0usize..5, 0i64..2 * BAR, 1i64..BAR).prop_map(|(t, s, l)| Op::Comp(t, s, s + l)),
        2 => (0usize..5).prop_map(Op::Delete),
        1 => Just(Op::Flatten),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Any run of take commands undoes back to the start, redoes to the end, and every state
    /// saves, loads and saves to the same text with every comp clip in the lane.
    #[test]
    fn random_take_edits_undo_redo_and_round_trip(ops in prop::collection::vec(op(), 1..12)) {
        let reg = registry();
        let mut d = four_takes(&reg);
        let start = save_project(d.doc());
        let mut applied = 0;
        let mut states = vec![start.clone()];
        for o in ops {
            let cmd = match o {
                Op::Pick(t) => SongCommand::PickTake { group: 1, take: t },
                Op::Comp(t, s, e) => SongCommand::SetCompSegment { group: 1, take: t, start: s, end: e },
                Op::Delete(t) => SongCommand::DeleteTake { group: 1, take: t },
                Op::Flatten => SongCommand::FlattenTakes { group: 1 },
            };
            if d.apply(sc(cmd), &reg).is_ok() {
                applied += 1;
                let text = save_project(d.doc());
                let loaded = load_project(&text, &reg).expect("every state loads");
                prop_assert_eq!(save_project(&loaded), text.clone());
                for (_, _, g) in d.doc().song.take_groups() {
                    prop_assert!(g.clips.iter().all(|c| d.doc().song.clip(*c).is_some()));
                    prop_assert!(g.comp.windows(2).all(|w| w[0].end <= w[1].start));
                }
                states.push(text);
            }
        }
        for want in states.iter().rev().skip(1) {
            d.undo(&reg).unwrap();
            prop_assert_eq!(&save_project(d.doc()), want);
        }
        prop_assert_eq!(applied + 1, states.len());
        for want in states.iter().skip(1) {
            d.redo(&reg).unwrap();
            prop_assert_eq!(&save_project(d.doc()), want);
        }
    }
}
