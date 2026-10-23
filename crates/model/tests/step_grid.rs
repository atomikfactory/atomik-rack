//! The step grid on note clips: validation, `SetClipGrid`, undo,
//! merge, batches with the clip length, swing in `flatten_notes` and project
//! format v5.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::flatten_notes;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, Curve, Document, Lane, LaneKind, ModelError, Note,
    SongCommand, StepGrid, StepResolution, SyncOp, Track, load_project, save_project,
};
use common::audio::{add_audio_clip, add_audio_track, audio_registry, register};
use common::song::*;
use common::*;
use proptest::prelude::*;
use serde_json::{Value, json};

const RES: [StepResolution; 3] = [
    StepResolution::Eighth,
    StepResolution::Sixteenth,
    StepResolution::ThirtySecond,
];

fn grid(resolution: StepResolution, steps: u16, swing: u8) -> StepGrid {
    StepGrid {
        resolution,
        steps,
        swing,
    }
}

fn sixteenth(swing: u8) -> StepGrid {
    grid(StepResolution::Sixteenth, 16, swing)
}

fn set_grid(clip: ClipId, g: Option<StepGrid>, merge: Option<u64>) -> Command {
    sc(SongCommand::SetClipGrid {
        clip,
        grid: g,
        merge,
    })
}

fn geometry(c: &Clip, length: i64) -> Command {
    sc(SongCommand::SetClipGeometry {
        id: c.id,
        start: c.start,
        length,
        offset: c.offset,
        loop_length: c.loop_length,
        muted: c.muted,
        merge: None,
    })
}

fn rejects(d: &mut Document, cmd: Command, reg: &Registry) -> ModelError {
    let before = d.doc().clone();
    let undo = d.can_undo();
    let e = d.apply(cmd, reg).expect_err("command must be rejected");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
    assert_eq!(d.can_undo(), undo);
    e
}

/// A document with a track for `synth` and one note clip of one bar with two notes.
fn with_clip(reg: &Registry) -> (Document, ClipId) {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(
                0,
                BAR,
                vec![note(0, 100, 60, 0.5), note(3840, 100, 62, 0.8)],
            ),
        ),
        reg,
    )
    .unwrap();
    let id = d.last_clip_id().unwrap();
    (d, id)
}

fn clip_of(d: &Document, id: ClipId) -> Clip {
    d.doc().song.clip(id).unwrap().2.clone()
}

fn with_automation_clip(d: &mut Document, reg: &Registry) -> ClipId {
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
        reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "synth",
            1,
            auto_clip(0, BAR, vec![point(0, 0.5, Curve::Step)]),
        ),
        reg,
    )
    .unwrap();
    d.last_clip_id().unwrap()
}

// ---- types ----------------------------------------------------------------------------------

#[test]
fn step_lengths_follow_the_core_ppq() {
    assert_eq!(atomik_core::PPQ, 15_360);
    assert_eq!(RES.map(StepResolution::ticks), [7_680, 3_840, 1_920]);
    assert_eq!(StepResolution::Sixteenth.ticks(), atomik_core::PPQ / 4);
}

#[test]
fn a_grid_validates_steps_and_swing() {
    for r in RES {
        assert!(grid(r, 16, 0).validate().is_ok());
        assert!(grid(r, 64, 75).validate().is_ok());
        assert!(grid(r, 15, 0).validate().is_err());
        assert!(grid(r, 65, 0).validate().is_err());
        assert!(grid(r, 16, 76).validate().is_err());
    }
}

#[test]
fn the_swing_delay_is_half_a_step_times_percent_rounded_half_up() {
    let g = |r, s| grid(r, 16, s).swing_ticks();
    assert_eq!(g(StepResolution::Sixteenth, 0), 0);
    assert_eq!(g(StepResolution::Sixteenth, 50), 960);
    assert_eq!(g(StepResolution::Sixteenth, 75), 1_440);
    assert_eq!(g(StepResolution::Eighth, 50), 1_920);
    assert_eq!(g(StepResolution::Eighth, 75), 2_880);
    assert_eq!(g(StepResolution::ThirtySecond, 50), 480);
    assert_eq!(g(StepResolution::ThirtySecond, 75), 720);
    // 1 % of half of 1920 is 9.6, rounds to 10; 3 % is 28.8, rounds to 29; 5 % is exactly 48.
    assert_eq!(g(StepResolution::ThirtySecond, 1), 10);
    assert_eq!(g(StepResolution::ThirtySecond, 3), 29);
    assert_eq!(g(StepResolution::ThirtySecond, 5), 48);
    // Exactly .5 rounds up: 1 % of half of 100 would be 0.5 (not reachable with these steps,
    // but the formula is (swing * s + 100) / 200 for every s: 25 % of 3840 / 2 = 480 exactly).
    assert_eq!(g(StepResolution::Sixteenth, 25), 480);
}

// ---- SetClipGrid ----------------------------------------------------------------------------

#[test]
fn set_clip_grid_sets_clears_and_undoes() {
    let reg = registry();
    let (mut d, id) = with_clip(&reg);
    assert_eq!(clip_of(&d, id).grid, None, "no grid by default");
    let g = sixteenth(30);
    let before = clip_of(&d, id);
    assert_eq!(
        d.apply(set_grid(id, Some(g), None), &reg).unwrap(),
        [SyncOp::SongChanged]
    );
    let c = clip_of(&d, id);
    assert_eq!(c.grid, Some(g));
    assert_eq!(
        (c.start, c.length, c.offset, &c.content),
        (before.start, before.length, before.offset, &before.content),
        "setting a grid changes neither the length nor the notes"
    );
    d.apply(set_grid(id, None, None), &reg).unwrap();
    assert_eq!(clip_of(&d, id).grid, None);
    assert_eq!(d.undo(&reg).unwrap(), [SyncOp::SongChanged]);
    assert_eq!(clip_of(&d, id).grid, Some(g));
    d.undo(&reg).unwrap();
    assert_eq!(clip_of(&d, id), before);
    d.redo(&reg).unwrap();
    assert_eq!(clip_of(&d, id).grid, Some(g));
}

#[test]
fn set_clip_grid_rejections() {
    let reg = registry();
    let (mut d, id) = with_clip(&reg);
    for bad in [
        grid(StepResolution::Sixteenth, 15, 0),
        grid(StepResolution::Sixteenth, 65, 0),
        grid(StepResolution::Sixteenth, 0, 0),
        grid(StepResolution::Sixteenth, 16, 76),
    ] {
        let e = rejects(&mut d, set_grid(id, Some(bad), None), &reg);
        assert!(matches!(e, ModelError::OutOfRange(_)), "{bad:?}: {e:?}");
    }
    let e = rejects(&mut d, set_grid(ClipId(999), None, None), &reg);
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
    // An automation clip takes no grid, neither by command nor with AddClip.
    let auto = with_automation_clip(&mut d, &reg);
    let e = rejects(&mut d, set_grid(auto, Some(sixteenth(0)), None), &reg);
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    let mut c = auto_clip(BAR, BAR, vec![]);
    c.grid = Some(sixteenth(0));
    rejects(&mut d, add_clip("synth", 1, c), &reg);
    // A note clip added with a bad grid is rejected, with a good one it keeps it.
    let mut c = notes_clip(BAR, BAR, vec![]);
    c.grid = Some(grid(StepResolution::Sixteenth, 99, 0));
    rejects(&mut d, add_clip("synth", 0, c), &reg);
    let mut c = notes_clip(BAR, BAR, vec![]);
    c.grid = Some(sixteenth(10));
    d.apply(add_clip("synth", 0, c), &reg).unwrap();
    assert_eq!(
        clip_of(&d, d.last_clip_id().unwrap()).grid,
        Some(sixteenth(10))
    );
}

#[test]
fn an_audio_clip_takes_no_grid() {
    let reg = audio_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(register(1, 1000), &reg).unwrap();
    d.apply(add_audio_track(), &reg).unwrap();
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = d.last_clip_id().unwrap();
    assert_eq!(clip_of(&d, id).grid, None);
    let e = rejects(&mut d, set_grid(id, Some(sixteenth(0)), None), &reg);
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn swing_drags_merge_into_one_undo_step() {
    let reg = registry();
    let (mut d, id) = with_clip(&reg);
    d.apply(set_grid(id, Some(sixteenth(0)), None), &reg)
        .unwrap();
    for swing in [10, 20, 30, 40] {
        d.apply(set_grid(id, Some(sixteenth(swing)), Some(7)), &reg)
            .unwrap();
    }
    assert_eq!(clip_of(&d, id).grid.unwrap().swing, 40);
    d.undo(&reg).unwrap();
    assert_eq!(
        clip_of(&d, id).grid.unwrap().swing,
        0,
        "the drag was one step"
    );
    d.undo(&reg).unwrap();
    assert_eq!(clip_of(&d, id).grid, None);
    // A different merge id starts a new step.
    d.redo(&reg).unwrap();
    d.apply(set_grid(id, Some(sixteenth(5)), Some(1)), &reg)
        .unwrap();
    d.apply(set_grid(id, Some(sixteenth(6)), Some(2)), &reg)
        .unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(clip_of(&d, id).grid.unwrap().swing, 5);
}

#[test]
fn the_grid_survives_content_and_geometry_edits_split_and_duplicate() {
    let reg = registry();
    let (mut d, id) = with_clip(&reg);
    let g = grid(StepResolution::ThirtySecond, 32, 20);
    d.apply(set_grid(id, Some(g), None), &reg).unwrap();
    d.apply(
        sc(SongCommand::SetClipContent {
            id,
            content: ClipContent::Notes(vec![note(5, 5, 40, 1.0)]),
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(clip_of(&d, id).grid, Some(g));
    d.apply(geometry(&clip_of(&d, id), 2 * BAR), &reg).unwrap();
    assert_eq!(clip_of(&d, id).grid, Some(g));
    d.apply(sc(SongCommand::DuplicateClip { id, start: 3 * BAR }), &reg)
        .unwrap();
    assert_eq!(clip_of(&d, d.last_clip_id().unwrap()).grid, Some(g));
    d.apply(sc(SongCommand::SplitClip { id, at: BAR }), &reg)
        .unwrap();
    assert_eq!(clip_of(&d, d.last_clip_id().unwrap()).grid, Some(g));
    assert_eq!(clip_of(&d, id).grid, Some(g));
}

// ---- Batch ----------------------------------------------------------------------------------

#[test]
fn a_batch_sets_the_grid_and_the_length_as_one_undo_step() {
    let reg = registry();
    let (mut d, id) = with_clip(&reg);
    let before = d.doc().clone();
    let c = clip_of(&d, id);
    let g = grid(StepResolution::ThirtySecond, 32, 25);
    let ops = d
        .apply(
            Command::Batch(vec![set_grid(id, Some(g), Some(3)), geometry(&c, 2 * BAR)]),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [SyncOp::SongChanged], "one change report");
    let after = clip_of(&d, id);
    assert_eq!((after.grid, after.length), (Some(g), 2 * BAR));
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "one undo restores both");
    d.redo(&reg).unwrap();
    assert_eq!(clip_of(&d, id), after);
    // Later commands see the earlier ones; the order of geometry and grid does not matter.
    d.undo(&reg).unwrap();
    d.apply(
        Command::Batch(vec![geometry(&c, BAR / 2), set_grid(id, Some(g), None)]),
        &reg,
    )
    .unwrap();
    assert_eq!(clip_of(&d, id).length, BAR / 2);
    assert_eq!(clip_of(&d, id).grid, Some(g));
}

#[test]
fn a_batch_is_all_or_nothing_and_takes_only_cables_clip_grid_and_clip_geometry() {
    let reg = registry();
    let (mut d, id) = with_clip(&reg);
    let c = clip_of(&d, id);
    // A bad second command leaves the first undone.
    let e = rejects(
        &mut d,
        Command::Batch(vec![
            set_grid(id, Some(sixteenth(0)), None),
            set_grid(id, Some(grid(StepResolution::Sixteenth, 99, 0)), None),
        ]),
        &reg,
    );
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    let e = rejects(
        &mut d,
        Command::Batch(vec![
            set_grid(id, Some(sixteenth(0)), None),
            geometry(&c, 0),
        ]),
        &reg,
    );
    assert!(matches!(e, ModelError::OutOfRange(_)), "{e:?}");
    // Other song commands and rack commands are not batchable.
    for other in [
        sc(SongCommand::RemoveClip { id }),
        sc(SongCommand::SetSongEnd(BAR)),
        Command::SetTempo(100.0),
    ] {
        let e = rejects(
            &mut d,
            Command::Batch(vec![set_grid(id, Some(sixteenth(0)), None), other]),
            &reg,
        );
        assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
    }
    rejects(&mut d, Command::Batch(Vec::new()), &reg);
}

#[test]
fn a_batch_mixing_cables_and_clip_edits_reports_each_change_once() {
    let reg = registry();
    let (mut d, id) = with_clip(&reg);
    let cable = d.doc().cables[0].clone();
    let ops = d
        .apply(
            Command::Batch(vec![
                Command::Disconnect { to: cable.to },
                set_grid(id, Some(grid(StepResolution::Eighth, 16, 10)), None),
            ]),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [SyncOp::RoutingChanged, SyncOp::SongChanged]);
    d.undo(&reg).unwrap();
    assert_eq!(clip_of(&d, id).grid, None);
}

// ---- swing in flatten ------------------------------------------------------------------------

fn track_of(clips: Vec<Clip>) -> Track {
    Track {
        device: key("synth"),
        muted: false,
        solo: false,
        armed: false,
        lanes: vec![Lane {
            kind: LaneKind::Notes,
            clips: clips
                .into_iter()
                .enumerate()
                .map(|(i, mut c)| {
                    c.id = ClipId(i as u64 + 1);
                    c
                })
                .collect(),
        }],
    }
}

fn gridded(start: i64, length: i64, g: Option<StepGrid>, notes: Vec<Note>) -> Clip {
    let mut c = notes_clip(start, length, notes);
    c.grid = g;
    c
}

/// The flattened `(tick, length)` of each note of a clip.
fn flat(clip: Clip) -> Vec<(i64, i64)> {
    flatten_notes(&track_of(vec![clip]))
        .into_iter()
        .map(|(t, l, _, _)| (t, l))
        .collect()
}

#[test]
fn swing_delays_odd_steps_only_for_every_resolution_and_swing() {
    for r in RES {
        let s = r.ticks();
        // The start of steps 0..4, then one tick into steps 4 and 5.
        let notes: Vec<Note> = (0..4)
            .map(|i| note(i * s, 10, 60 + i as u8, 0.5))
            .chain([note(4 * s + 1, 10, 80, 0.5), note(5 * s + 1, 10, 81, 0.5)])
            .collect();
        for swing in [0u8, 1, 37, 50, 75] {
            let d = (i64::from(swing) * s + 100) / 200;
            let got = flat(gridded(0, 16 * s, Some(grid(r, 16, swing)), notes.clone()));
            let want = vec![
                (0, 10),
                (s + d, 10),
                (2 * s, 10),
                (3 * s + d, 10),
                (4 * s + 1, 10),
                (5 * s + 1 + d, 10),
            ];
            assert_eq!(got, want, "{r:?} swing {swing}");
        }
    }
}

#[test]
fn swing_table_for_sixteenths() {
    // (note tick, swing, flattened tick); s = 3840.
    let cases = [
        (0, 50, 0),
        (3839, 50, 3839),
        (3840, 0, 3840),
        (3840, 50, 3840 + 960),
        (3840, 75, 3840 + 1440),
        (7679, 75, 7679 + 1440),
        (7680, 75, 7680),
        (11_520, 50, 11_520 + 960),
        (11_519, 50, 11_519),
    ];
    for (tick, swing, want) in cases {
        let c = gridded(
            0,
            4 * BAR,
            Some(sixteenth(swing)),
            vec![note(tick, 100, 60, 1.0)],
        );
        assert_eq!(flat(c), [(want, 100)], "tick {tick} swing {swing}");
    }
}

#[test]
fn a_clip_without_a_grid_plays_unchanged() {
    let c = gridded(0, BAR, None, vec![note(3840, 100, 60, 1.0)]);
    assert_eq!(flat(c), [(3840, 100)]);
}

#[test]
fn the_step_parity_is_counted_from_the_content_start_not_the_clip_position() {
    // The clip sits at song tick 1000, not on a step: the steps still count from its content.
    let c = gridded(
        1000,
        BAR,
        Some(sixteenth(50)),
        vec![note(0, 10, 60, 1.0), note(3840, 10, 61, 1.0)],
    );
    assert_eq!(flat(c), [(1000, 10), (1000 + 3840 + 960, 10)]);
}

#[test]
fn swing_follows_content_ticks_with_a_clip_offset() {
    let notes = vec![
        note(0, 10, 60, 1.0),
        note(3840, 10, 61, 1.0),
        note(7680, 10, 62, 1.0),
    ];
    // Offset 3840: content tick 3840 (an odd step) is shown at the clip start and is swung;
    // the note at 0 is before the offset and never plays; 7680 is an even step.
    let mut c = gridded(0, BAR, Some(sixteenth(50)), notes);
    c.offset = 3840;
    assert_eq!(flat(c), [(960, 10), (3840, 10)]);
    // An offset that is not a step keeps the parity of the content, not of the song.
    let mut c = gridded(
        0,
        BAR,
        Some(sixteenth(50)),
        vec![note(2000, 10, 60, 1.0), note(4000, 10, 61, 1.0)],
    );
    c.offset = 1000;
    assert_eq!(flat(c), [(1000, 10), (3000 + 960, 10)]);
}

#[test]
fn a_note_before_the_offset_is_not_swung_into_the_window() {
    // Content tick 3840 swings to 5280, but the window starts at 5000: the note belongs to
    // the part before the offset, so it does not play (swing never pulls a note in).
    let mut c = gridded(0, BAR, Some(sixteenth(75)), vec![note(3840, 10, 60, 1.0)]);
    c.offset = 5000;
    assert_eq!(flat(c), []);
}

#[test]
fn loops_swing_every_pass_alike() {
    let mut c = gridded(
        0,
        3 * 7680,
        Some(sixteenth(50)),
        vec![note(0, 10, 60, 1.0), note(3840, 10, 61, 1.0)],
    );
    c.loop_length = Some(7680);
    assert_eq!(
        flat(c),
        [
            (0, 10),
            (4800, 10),
            (7680, 10),
            (7680 + 4800, 10),
            (15_360, 10),
            (15_360 + 4800, 10)
        ]
    );
}

#[test]
fn a_loop_of_an_odd_number_of_steps_swings_by_content_tick_on_every_pass() {
    // A loop of 3 sixteenth steps: the content window [0, 11520) repeats and the content
    // parity is the same on every pass because the content tick is what is swung.
    let mut c = gridded(
        0,
        2 * 11_520,
        Some(sixteenth(50)),
        vec![note(3840, 10, 60, 1.0), note(7680, 10, 61, 1.0)],
    );
    c.loop_length = Some(11_520);
    assert_eq!(
        flat(c),
        [
            (4800, 10),
            (7680, 10),
            (11_520 + 4800, 10),
            (11_520 + 7680, 10)
        ]
    );
}

#[test]
fn a_swung_note_pushed_past_the_clip_end_is_dropped_and_others_are_cut() {
    // The clip ends at 5000. The note at 3840 swings to 5280, at or after the end: dropped,
    // like any note that starts at or after the clip end. The even-step note at 3000 is
    // unmoved and cut at 5000.
    let c = gridded(
        0,
        5000,
        Some(sixteenth(75)),
        vec![note(3000, 3000, 60, 1.0), note(3840, 10, 61, 1.0)],
    );
    assert_eq!(flat(c), [(3000, 2000)]);
    // A swung note that still starts inside is cut at the clip end: 3840 + 1440 = 5280.
    let c = gridded(
        0,
        5400,
        Some(sixteenth(75)),
        vec![note(3840, 1000, 61, 1.0)],
    );
    assert_eq!(flat(c), [(5280, 120)]);
}

#[test]
fn a_swung_note_pushed_past_the_loop_end_is_dropped_on_every_pass() {
    let mut c = gridded(
        0,
        3 * 5000,
        Some(sixteenth(75)),
        vec![note(100, 10, 61, 1.0), note(3840, 10, 60, 1.0)],
    );
    c.loop_length = Some(5000); // 3840 + 1440 = 5280 >= 5000
    assert_eq!(flat(c), [(100, 10), (5100, 10), (10_100, 10)]);
}

#[test]
fn a_clip_that_a_later_clip_takes_over_cuts_and_drops_swung_notes_like_any_note() {
    let early = gridded(
        0,
        BAR,
        Some(sixteenth(50)),
        vec![note(3840, 2000, 60, 1.0), note(7000, 10, 62, 1.0)],
    );
    let later = gridded(5000, BAR, None, vec![note(0, 10, 70, 1.0)]);
    // The early clip is audible on [0, 5000): the note at 3840 swings to 4800 and is cut at
    // 5000; the note at 7000 is hidden.
    let got = flatten_notes(&track_of(vec![early, later]));
    assert_eq!(got, [(4800, 200, 60, 1.0), (5000, 10, 70, 1.0)]);
}

proptest! {
    #[test]
    fn swing_never_moves_a_note_early_and_by_at_most_half_a_step(
        tick in 0i64..2_000_000,
        res in 0usize..3,
        swing in 0u8..=75,
    ) {
        let g = grid(RES[res], 16, swing);
        let s = g.resolution.ticks();
        let swung = g.swung(tick);
        prop_assert!(swung >= tick);
        prop_assert!(swung - tick <= s / 2);
        if tick.rem_euclid(2 * s) < s {
            prop_assert_eq!(swung, tick);
        } else {
            prop_assert_eq!(swung - tick, (i64::from(swing) * s + 100) / 200);
        }
        // Flattening applies exactly that, with the length unchanged.
        let c = gridded(0, 4_000_000, Some(g), vec![note(tick, 7, 60, 1.0)]);
        prop_assert_eq!(flat(c), vec![(swung, 7)]);
    }
}

// ---- project format v5 -----------------------------------------------------------------------

fn doc_with_grids(reg: &Registry) -> Document {
    let (mut d, id) = with_clip(reg);
    d.apply(
        set_grid(id, Some(grid(StepResolution::ThirtySecond, 48, 33)), None),
        reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(2 * BAR, BAR, vec![note(0, 10, 36, 0.25)]),
        ),
        reg,
    )
    .unwrap();
    let mut c = notes_clip(4 * BAR, BAR, vec![]);
    c.grid = Some(grid(StepResolution::Eighth, 64, 75));
    d.apply(add_clip("synth", 0, c), reg).unwrap();
    d
}

#[test]
fn v5_writes_grid_only_when_present() {
    let reg = registry();
    let d = doc_with_grids(&reg);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 14);
    let clips = &v["song"]["tracks"][0]["lanes"][0]["clips"];
    assert_eq!(
        clips[0]["grid"],
        json!({"resolution": "thirty_second", "steps": 48, "swing": 33})
    );
    assert!(clips[1].get("grid").is_none(), "no grid, no field");
    assert_eq!(
        clips[2]["grid"],
        json!({"resolution": "eighth", "steps": 64, "swing": 75})
    );
    assert!(
        text.contains(
            "\"grid\": {\"resolution\": \"thirty_second\", \"steps\": 48, \"swing\": 33}"
        )
    );
}

#[test]
fn v5_round_trips_bit_exactly_and_canonically() {
    let reg = registry();
    let doc = doc_with_grids(&reg).doc().clone();
    let text = save_project(&doc);
    let back = load_project(&text, &reg).unwrap();
    assert_eq!(back, doc);
    assert_eq!(save_project(&back), text);
}

fn edited(text: &str, f: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    f(&mut v);
    serde_json::to_string_pretty(&v).unwrap()
}

#[test]
fn older_versions_load_without_grids_and_reject_the_field() {
    let reg = registry();
    let (d, _) = with_clip(&reg);
    let text = save_project(d.doc());
    assert!(!text.contains("grid"));
    for version in 3..=4 {
        let old = text.replace("\"version\": 14", &format!("\"version\": {version}"));
        let back = load_project(&old, &reg).unwrap();
        assert!(
            back.song
                .tracks
                .iter()
                .flat_map(|t| &t.lanes)
                .flat_map(|l| &l.clips)
                .all(|c| c.grid.is_none()),
            "v{version}"
        );
    }
    // A grid in an older file is an unknown field.
    let with = save_project(doc_with_grids(&reg).doc());
    for version in 3..=4 {
        let old = with.replace("\"version\": 14", &format!("\"version\": {version}"));
        let e = load_project(&old, &reg).unwrap_err().to_string();
        assert!(e.contains("grid"), "v{version}: {e}");
    }
}

#[test]
fn bad_grids_are_rejected_with_their_path() {
    let reg = registry();
    let text = save_project(doc_with_grids(&reg).doc());
    let cases = [
        (
            json!({"resolution": "sixteenth", "steps": 15, "swing": 0}),
            "steps",
        ),
        (
            json!({"resolution": "sixteenth", "steps": 16, "swing": 76}),
            "swing",
        ),
        (
            json!({"resolution": "quarter", "steps": 16, "swing": 0}),
            "resolution",
        ),
        (json!({"resolution": "sixteenth", "steps": 16}), "swing"),
        (
            json!({"resolution": "sixteenth", "steps": 16, "swing": 0, "x": 1}),
            "unknown field",
        ),
        (json!(null), "grid"),
    ];
    for (i, (grid, what)) in cases.into_iter().enumerate() {
        let bad = edited(&text, |v| {
            v["song"]["tracks"][0]["lanes"][0]["clips"][0]["grid"] = grid;
        });
        let e = load_project(&bad, &reg).unwrap_err().to_string();
        assert!(e.contains("clips[0].grid"), "case {i}: {e}");
        assert!(e.contains(what), "case {i}: {e}");
    }
}

#[test]
fn an_automation_clip_cannot_carry_a_grid_in_a_file() {
    let reg = registry();
    let (mut d, _) = with_clip(&reg);
    with_automation_clip(&mut d, &reg);
    let text = save_project(d.doc());
    let bad = edited(&text, |v| {
        v["song"]["tracks"][0]["lanes"][1]["clips"][0]["grid"] =
            json!({"resolution": "sixteenth", "steps": 16, "swing": 0});
    });
    let e = load_project(&bad, &reg).unwrap_err().to_string();
    assert!(e.contains("unknown field") && e.contains("grid"), "{e}");
}
