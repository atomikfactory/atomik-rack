//! StepEditorView behaviour through its public contract: hit testing of
//! cells, headers and the toolbar, toggle / paint / erase / velocity drags, grid and length
//! changes as one `Batch`, swing drags, audition, drops, open / close and pad names. Every
//! emitted command is applied to a real model `Document`, so the model's own validation
//! judges it.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{ClipContent, ClipId, Command, Document, Note, RackDoc, SongCommand};
use atomik_model::{StepGrid, StepResolution, pad_key};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::step_editor::{DEFAULT_GRID, VELOCITY_DRAG_PIXELS};
use atomik_ui::{DisplayList, Key, Modifiers, StepEditorView, UiAction, UiEvent, VelocityLevel};
use proptest::prelude::*;
use std::collections::BTreeSet;
use std::path::PathBuf;
use support::drum::*;
use support::song::{press, run};
use support::*;

fn setup_with(doc: RackDoc, clip: Option<ClipId>) -> (Document, Registry, StepEditorView) {
    let reg = registry();
    let v = editor(&doc, &reg, clip);
    (Document::new(doc), reg, v)
}

fn setup() -> (Document, Registry, StepEditorView) {
    setup_with(drum_doc(), Some(CLIP))
}

fn feed(v: &mut StepEditorView, d: &Document, reg: &Registry, events: &[UiEvent]) -> Vec<UiAction> {
    run(|e| v.handle(e, d.doc(), reg), events)
}

macro_rules! ck {
    ($v:ident, $d:ident, $reg:ident, $p:expr) => {{
        let p = $p;
        click(&mut $v, &$d, &$reg, p)
    }};
}

fn click(v: &mut StepEditorView, d: &Document, reg: &Registry, p: Point) -> Vec<UiAction> {
    feed(v, d, reg, &[down(p), up(p)])
}

/// Press at `points[0]`, move through the rest, release at the last point.
fn gesture(
    v: &mut StepEditorView,
    d: &Document,
    reg: &Registry,
    points: &[Point],
) -> Vec<UiAction> {
    let mut events = vec![down(points[0])];
    events.extend(points[1..].iter().map(|p| mv(*p)));
    events.push(up(*points.last().unwrap()));
    feed(v, d, reg, &events)
}

fn resync(v: &mut StepEditorView, d: &Document, reg: &Registry) {
    let _ = v.handle(&mv(Point::new(-50.0, -50.0)), d.doc(), reg);
}

fn texts(v: &mut StepEditorView, d: &Document, reg: &Registry) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), reg);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn v7(v: f32) -> u8 {
    (v * 127.0).round() as u8
}

fn ends(a: &[UiAction]) -> usize {
    a.iter().filter(|x| **x == UiAction::EndMerge).count()
}

fn notes_of(d: &Document) -> Vec<Note> {
    match clip_of(d.doc()).content {
        ClipContent::Notes(n) => n,
        _ => panic!("note clip"),
    }
}

fn beat_notes() -> Vec<Note> {
    match beat_clip().content {
        ClipContent::Notes(n) => n,
        _ => Vec::new(),
    }
}

fn wheel(pos: Point, dx: f64, dy: f64) -> UiEvent {
    UiEvent::Wheel {
        delta: Vec2::new(dx, dy),
        pos,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn rows_run_pad_1_at_the_bottom_and_steps_left_to_right() {
    let (_, _, v) = setup();
    assert!(v.is_present());
    assert_eq!(v.clip(), Some(CLIP));
    assert!(
        v.header_center(1).y > v.header_center(2).y,
        "pad 1 is the bottom row"
    );
    assert!(v.header_center(16).y < v.header_center(15).y);
    assert!(v.cell_center(1, 0).x < v.cell_center(1, 1).x);
    assert!((v.cell_center(1, 3).y - v.header_center(1).y).abs() < 1e-9);
    assert!(
        v.cell_center(1, 0).x > v.header_center(1).x,
        "cells right of the headers"
    );
    let (a, b) = (v.cell_rect(1, 0), v.cell_rect(1, 1));
    assert!((a.x1 - b.x0).abs() < 1e-9 && (a.width() - b.width()).abs() < 1e-9);
}

#[test]
fn the_grid_and_the_cells_come_from_the_clip() {
    let (_, _, v) = setup();
    assert_eq!(v.grid(), grid(StepResolution::Sixteenth, 16, 0));
    assert!(v.has_grid());
    for k in [0, 4, 8, 12] {
        assert_eq!(v.cell_velocity(1, k).map(v7), Some(80), "kick on {k}");
    }
    assert_eq!(v.cell_velocity(1, 1), None);
    assert_eq!(v.cell_velocity(2, 4).map(v7), Some(120));
    assert_eq!(v.cell_velocity(6, 2).map(v7), Some(40));
    assert_eq!(v.cell_velocity(6, 3), None);
    assert_eq!(v.off_grid_count(), 0);
    assert_eq!(
        v.pads_with_steps().into_iter().collect::<Vec<_>>(),
        vec![1, 2, 6]
    );
}

#[test]
fn pad_names_are_file_names_else_note_names_with_c4_equals_60() {
    let (_, _, v) = setup();
    assert_eq!(v.pad_name(1), "kick_808.wav");
    assert_eq!(
        v.pad_name(2),
        "snare_soft.flac",
        "the first zone of the pad"
    );
    assert_eq!(v.pad_name(3), "D2");
    assert_eq!(v.pad_name(16), "D#3");
    assert!(v.pad_has_sound(1) && v.pad_has_sound(2) && !v.pad_has_sound(3));
    // A pad whose asset is not in the pool falls back to the note name.
    let mut doc = drum_doc();
    doc.assets.remove(KICK);
    let (_, _, v) = setup_with(doc, Some(CLIP));
    assert_eq!(v.pad_name(1), "C2");
    let (d, reg, mut v) = setup();
    let t = texts(&mut v, &d, &reg);
    for s in [
        "01",
        "kick_808.wav",
        "16",
        "D#3",
        "STEP EDITOR",
        "SOFT",
        "MED",
        "HARD",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
}

#[test]
fn clicking_an_empty_cell_adds_a_one_step_note_at_the_default_level() {
    let (mut d, reg, mut v) = setup();
    assert_eq!(v.default_level(), VelocityLevel::Medium);
    let a = ck!(v, d, reg, v.cell_center(3, 5));
    let lists = note_lists(&a);
    assert_eq!(lists.len(), 1, "{a:?}");
    assert!(lists[0].1.is_some(), "merged per gesture");
    assert_eq!(ends(&a), 1);
    let before = notes_of(&d);
    apply_all(&mut d, &reg, &a);
    let after = notes_of(&d);
    assert_eq!(after.len(), before.len() + 1);
    let n = after
        .iter()
        .find(|n| n.key == pad_key(3))
        .expect("the new note");
    assert_eq!((n.tick, n.length, v7(n.velocity)), (5 * S16, S16, 80));
    assert!(
        after
            .windows(2)
            .all(|w| (w[0].tick, w[0].key) <= (w[1].tick, w[1].key))
    );
    resync(&mut v, &d, &reg);
    assert_eq!(v.cell_velocity(3, 5).map(v7), Some(80));
}

#[test]
fn the_toolbar_level_sets_the_velocity_of_new_steps() {
    let (mut d, reg, mut v) = setup();
    for (level, expected, step) in [
        (VelocityLevel::Soft, 40, 1),
        (VelocityLevel::Medium, 80, 3),
        (VelocityLevel::Hard, 120, 5),
    ] {
        let a = ck!(v, d, reg, v.level_button_center(level));
        assert!(a.is_empty(), "a level button is view state: {a:?}");
        assert_eq!(v.default_level(), level);
        assert_eq!(level.velocity7(), expected);
        let a = ck!(v, d, reg, v.cell_center(4, step));
        apply_all(&mut d, &reg, &a);
        resync(&mut v, &d, &reg);
        assert_eq!(v.cell_velocity(4, step).map(v7), Some(expected));
    }
    assert_eq!(
        VelocityLevel::of_velocity(40.0 / 127.0),
        VelocityLevel::Soft
    );
    assert_eq!(
        VelocityLevel::of_velocity(80.0 / 127.0),
        VelocityLevel::Medium
    );
    assert_eq!(
        VelocityLevel::of_velocity(120.0 / 127.0),
        VelocityLevel::Hard
    );
}

#[test]
fn clicking_an_on_cell_removes_its_notes_and_nothing_else() {
    let (mut d, reg, mut v) = setup();
    let before = notes_of(&d);
    let a = ck!(v, d, reg, v.cell_center(1, 4));
    assert_eq!(note_lists(&a).len(), 1, "{a:?}");
    assert_eq!(ends(&a), 1);
    apply_all(&mut d, &reg, &a);
    let after = notes_of(&d);
    assert_eq!(after.len(), before.len() - 1);
    assert!(
        !after
            .iter()
            .any(|n| n.key == pad_key(1) && n.tick == 4 * S16)
    );
    resync(&mut v, &d, &reg);
    assert_eq!(v.cell_velocity(1, 4), None);
}

#[test]
fn a_note_inside_a_step_but_not_on_its_start_counts_as_on_and_is_removed_with_it() {
    let mut clip = beat_clip();
    if let ClipContent::Notes(n) = &mut clip.content {
        n.push(Note {
            tick: 9 * S16 + 700,
            length: 500,
            key: pad_key(4),
            velocity: 0.5,
        });
        n.sort_by_key(|n| (n.tick, n.key));
    }
    let (mut d, reg, mut v) = setup_with(drum_doc_with(Some(clip)), Some(CLIP));
    assert!(v.cell_velocity(4, 9).is_some());
    assert_eq!(v.off_grid_count(), 0, "inside the grid counts as on-grid");
    let a = ck!(v, d, reg, v.cell_center(4, 9));
    apply_all(&mut d, &reg, &a);
    assert!(!notes_of(&d).iter().any(|n| n.key == pad_key(4)));
}

#[test]
fn dragging_across_empty_cells_paints_them_in_one_merge_group() {
    let (mut d, reg, mut v) = setup();
    let y = v.cell_center(3, 0).y;
    let pts: Vec<Point> = (0..=5)
        .map(|k| Point::new(v.cell_center(3, k).x, y))
        .collect();
    let a = gesture(&mut v, &d, &reg, &pts);
    let lists = note_lists(&a);
    assert!(lists.len() >= 2, "one list per change: {}", lists.len());
    let merge = lists[0].1;
    assert!(merge.is_some() && lists.iter().all(|(_, m)| *m == merge));
    assert_eq!(ends(&a), 1);
    apply_all(&mut d, &reg, &a);
    let row: Vec<i64> = notes_of(&d)
        .iter()
        .filter(|n| n.key == pad_key(3))
        .map(|n| n.tick / S16)
        .collect();
    assert_eq!(row, vec![0, 1, 2, 3, 4, 5]);
    // One undo removes the whole stroke.
    assert!(d.undo(&reg).is_some());
    assert!(!notes_of(&d).iter().any(|n| n.key == pad_key(3)));
}

#[test]
fn painting_leaves_cells_that_are_already_on_alone() {
    let (mut d, reg, mut v) = setup();
    let y = v.cell_center(1, 0).y;
    // Press the empty step 1 and drag right over the empty cells and the kick on step 4.
    let pts: Vec<Point> = (1..=5)
        .map(|k| Point::new(v.cell_center(1, k).x, y))
        .collect();
    let a = gesture(&mut v, &d, &reg, &pts);
    apply_all(&mut d, &reg, &a);
    let kick: Vec<(i64, u8)> = notes_of(&d)
        .iter()
        .filter(|n| n.key == pad_key(1))
        .map(|n| (n.tick / S16, v7(n.velocity)))
        .collect();
    assert_eq!(
        kick,
        vec![
            (0, 80),
            (1, 80),
            (2, 80),
            (3, 80),
            (4, 80),
            (5, 80),
            (8, 80),
            (12, 80)
        ]
    );
}

#[test]
fn dragging_sideways_from_an_on_cell_erases_along_the_row() {
    let (mut d, reg, mut v) = setup();
    let y = v.cell_center(1, 0).y;
    let pts: Vec<Point> = (0..=8)
        .map(|k| Point::new(v.cell_center(1, k).x, y))
        .collect();
    let a = gesture(&mut v, &d, &reg, &pts);
    let lists = note_lists(&a);
    assert!(lists.iter().all(|(_, m)| *m == lists[0].1 && m.is_some()));
    assert_eq!(ends(&a), 1);
    apply_all(&mut d, &reg, &a);
    let kick: Vec<i64> = notes_of(&d)
        .iter()
        .filter(|n| n.key == pad_key(1))
        .map(|n| n.tick / S16)
        .collect();
    assert_eq!(kick, vec![12], "0, 4 and 8 erased, 12 untouched");
    assert!(
        notes_of(&d).iter().any(|n| n.key == pad_key(2)),
        "other rows untouched"
    );
}

#[test]
fn dragging_vertically_on_an_on_cell_changes_the_velocity_continuously() {
    let (mut d, reg, mut v) = setup();
    let p = v.cell_center(1, 4);
    let up_by = VELOCITY_DRAG_PIXELS * 0.25;
    let a = gesture(
        &mut v,
        &d,
        &reg,
        &[
            p,
            p + Vec2::new(0.0, -up_by / 2.0),
            p + Vec2::new(0.0, -up_by),
        ],
    );
    let lists = note_lists(&a);
    assert!(lists.len() >= 2, "continuous: {}", lists.len());
    assert!(
        lists.iter().all(|(_, m)| *m == lists[0].1 && m.is_some()),
        "one merge id"
    );
    assert_eq!(ends(&a), 1);
    let vels: Vec<f32> = lists
        .iter()
        .map(|(l, _)| {
            l.iter()
                .find(|n| n.key == pad_key(1) && n.tick == 4 * S16)
                .unwrap()
                .velocity
        })
        .collect();
    assert!(vels.windows(2).all(|w| w[1] > w[0]), "monotone: {vels:?}");
    apply_all(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    let v_end = v.cell_velocity(1, 4).unwrap();
    assert!((v_end - (80.0 / 127.0 + 0.25)).abs() < 1e-4, "{v_end}");
    assert_eq!(
        v.cell_velocity(1, 0).map(v7),
        Some(80),
        "other cells unchanged"
    );
    assert_eq!(notes_of(&d).len(), beat_notes().len());
    // Far down clamps to the smallest velocity, far up to 1.
    let p = v.cell_center(1, 4);
    let a = gesture(&mut v, &d, &reg, &[p, p + Vec2::new(0.0, 500.0)]);
    apply_all(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    assert_eq!(v7(v.cell_velocity(1, 4).unwrap()), 1);
    let p = v.cell_center(1, 4);
    let a = gesture(&mut v, &d, &reg, &[p, p + Vec2::new(0.0, -500.0)]);
    apply_all(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    assert!((v.cell_velocity(1, 4).unwrap() - 1.0).abs() < 1e-6);
}

#[test]
fn a_press_on_an_on_cell_with_no_movement_does_not_edit_until_release() {
    let (d, reg, mut v) = setup();
    let p = v.cell_center(1, 4);
    let a = feed(&mut v, &d, &reg, &[down(p)]);
    assert!(a.is_empty(), "{a:?}");
    assert!(v.is_dragging());
    let a = feed(&mut v, &d, &reg, &[up(p)]);
    assert_eq!(note_lists(&a).len(), 1);
    assert!(!v.is_dragging());
}

#[test]
fn escape_during_a_drag_restores_the_notes_of_the_press() {
    let (mut d, reg, mut v) = setup();
    let y = v.cell_center(3, 0).y;
    let at = |k| Point::new(v.cell_center(3, k).x, y);
    let events = [down(at(0)), mv(at(1)), mv(at(2))];
    let a = feed(&mut v, &d, &reg, &events);
    apply_all(&mut d, &reg, &a);
    assert!(notes_of(&d).iter().any(|n| n.key == pad_key(3)));
    let a = feed(&mut v, &d, &reg, &[press(Key::Escape, Modifiers::NONE)]);
    assert!(
        !a.contains(&UiAction::CloseStepEditor),
        "Esc cancels the drag first"
    );
    apply_all(&mut d, &reg, &a);
    assert_eq!(notes_of(&d), beat_notes());
    assert!(!v.is_dragging());
    // Now Esc closes.
    let a = feed(&mut v, &d, &reg, &[press(Key::Escape, Modifiers::NONE)]);
    assert!(a.contains(&UiAction::CloseStepEditor));
}

#[test]
fn a_resolution_change_is_one_batch_of_grid_and_length_and_moves_no_note() {
    let (mut d, reg, mut v) = setup();
    let a = ck!(
        v,
        d,
        reg,
        v.resolution_button_center(StepResolution::ThirtySecond)
    );
    let cmds = commands(&a);
    assert_eq!(cmds.len(), 1, "{a:?}");
    let Command::Batch(parts) = &cmds[0] else {
        panic!("a Batch: {:?}", cmds[0])
    };
    assert_eq!(parts.len(), 2);
    assert!(matches!(
        &parts[0],
        Command::Song(SongCommand::SetClipGrid { clip, grid: Some(g), merge: None })
            if *clip == CLIP && *g == grid(StepResolution::ThirtySecond, 16, 0)
    ));
    assert!(matches!(
        &parts[1],
        Command::Song(SongCommand::SetClipGeometry { id, start: 0, length, offset: 0, .. })
            if *id == CLIP && *length == 16 * S32
    ));
    let notes = notes_of(&d);
    apply_all(&mut d, &reg, &a);
    let clip = clip_of(d.doc());
    assert_eq!(clip.grid, Some(grid(StepResolution::ThirtySecond, 16, 0)));
    assert_eq!(clip.length, 16 * S32, "the length follows the grid");
    assert_eq!(notes_of(&d), notes, "no note moved");
    assert!(d.undo(&reg).is_some());
    let clip = clip_of(d.doc());
    assert_eq!(
        (clip.grid, clip.length),
        (beat_clip().grid, 16 * S16),
        "one undo step"
    );
    resync(&mut v, &d, &reg);
    assert_eq!(v.grid().resolution, StepResolution::Sixteenth);
}

#[test]
fn a_steps_change_is_one_batch_and_sets_the_length() {
    let (mut d, reg, mut v) = setup();
    for steps in [32u16, 64, 48, 16] {
        let a = ck!(v, d, reg, v.steps_button_center(steps));
        let cmds = commands(&a);
        assert_eq!(cmds.len(), 1, "{steps}: {a:?}");
        assert!(matches!(&cmds[0], Command::Batch(p) if p.len() == 2));
        apply_all(&mut d, &reg, &a);
        let clip = clip_of(d.doc());
        assert_eq!(clip.grid.unwrap().steps, steps);
        assert_eq!(clip.length, i64::from(steps) * S16);
        resync(&mut v, &d, &reg);
        assert_eq!(v.grid().steps, steps);
    }
}

#[test]
fn choosing_the_grid_the_clip_already_has_emits_nothing() {
    let (d, reg, mut v) = setup();
    let p = v.resolution_button_center(StepResolution::Sixteenth);
    assert!(click(&mut v, &d, &reg, p).is_empty());
    let p = v.steps_button_center(16);
    assert!(click(&mut v, &d, &reg, p).is_empty());
}

#[test]
fn shrinking_the_grid_never_deletes_notes() {
    let mut clip = beat_clip();
    clip.length = 32 * S16;
    clip.grid = Some(grid(StepResolution::Sixteenth, 32, 0));
    if let ClipContent::Notes(n) = &mut clip.content {
        n.push(step_note(20, 1, 0.5));
        n.push(Note {
            tick: 0,
            length: 100,
            key: 60,
            velocity: 0.5,
        });
        n.sort_by_key(|n| (n.tick, n.key));
    }
    let (mut d, reg, mut v) = setup_with(drum_doc_with(Some(clip)), Some(CLIP));
    assert_eq!(v.off_grid_count(), 1, "the key 60 note");
    let before = notes_of(&d);
    let a = ck!(v, d, reg, v.steps_button_center(16));
    apply_all(&mut d, &reg, &a);
    assert_eq!(
        notes_of(&d),
        before,
        "the step-20 note and the key 60 note stay"
    );
    resync(&mut v, &d, &reg);
    assert_eq!(
        v.off_grid_count(),
        2,
        "step 20 is now past the grid, key 60 is no pad"
    );
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|x| x == "2 off-grid"), "{t:?}");
}

#[test]
fn a_clip_without_a_grid_shows_the_default_and_writes_it_only_on_a_toolbar_edit() {
    let mut clip = beat_clip();
    clip.grid = None;
    let (mut d, reg, mut v) = setup_with(drum_doc_with(Some(clip)), Some(CLIP));
    assert_eq!(v.grid(), DEFAULT_GRID);
    assert!(!v.has_grid());
    // A note edit leaves the grid alone.
    let a = ck!(v, d, reg, v.cell_center(3, 5));
    assert!(
        commands(&a)
            .iter()
            .all(|c| matches!(c, Command::Song(SongCommand::SetClipContent { .. })))
    );
    apply_all(&mut d, &reg, &a);
    assert_eq!(clip_of(d.doc()).grid, None);
    resync(&mut v, &d, &reg);
    // A swing drag writes the complete grid.
    let p = v.swing_field_center();
    let a = gesture(&mut v, &d, &reg, &[p, p + Vec2::new(10.0, 0.0)]);
    apply_all(&mut d, &reg, &a);
    assert_eq!(
        clip_of(d.doc()).grid,
        Some(grid(StepResolution::Sixteenth, 16, 10))
    );
    assert_eq!(
        clip_of(d.doc()).length,
        16 * S16,
        "length untouched by a swing drag"
    );
}

#[test]
fn a_swing_drag_emits_grid_commands_under_one_merge_id() {
    let (mut d, reg, mut v) = setup();
    let p = v.swing_field_center();
    let a = gesture(
        &mut v,
        &d,
        &reg,
        &[
            p,
            p + Vec2::new(5.0, 0.0),
            p + Vec2::new(12.0, 0.0),
            p + Vec2::new(25.0, 3.0),
        ],
    );
    let grids: Vec<(StepGrid, Option<u64>)> = commands(&a)
        .into_iter()
        .map(|c| match c {
            Command::Song(SongCommand::SetClipGrid {
                grid: Some(g),
                merge,
                ..
            }) => (g, merge),
            other => panic!("only grid commands: {other:?}"),
        })
        .collect();
    assert_eq!(grids.len(), 3);
    assert!(grids[0].1.is_some() && grids.iter().all(|g| g.1 == grids[0].1));
    assert_eq!(
        grids.iter().map(|g| g.0.swing).collect::<Vec<_>>(),
        vec![5, 12, 25]
    );
    assert_eq!(ends(&a), 1);
    apply_all(&mut d, &reg, &a);
    assert_eq!(clip_of(d.doc()).grid.unwrap().swing, 25);
    assert!(d.undo(&reg).is_some(), "one undo step");
    assert_eq!(clip_of(d.doc()).grid.unwrap().swing, 0);
    resync(&mut v, &d, &reg);
    // Clamped to 0..=75; a second drag has a fresh id.
    let p = v.swing_field_center();
    let b = gesture(&mut v, &d, &reg, &[p, p + Vec2::new(400.0, 0.0)]);
    let last = commands(&b).pop().unwrap();
    assert!(matches!(
        last,
        Command::Song(SongCommand::SetClipGrid { grid: Some(StepGrid { swing: 75, .. }), merge, .. })
            if merge != grids[0].1
    ));
    apply_all(&mut d, &reg, &b);
    resync(&mut v, &d, &reg);
    let p = v.swing_field_center();
    let c = gesture(&mut v, &d, &reg, &[p, p + Vec2::new(-400.0, 0.0)]);
    apply_all(&mut d, &reg, &c);
    assert_eq!(clip_of(d.doc()).grid.unwrap().swing, 0);
}

#[test]
fn swing_double_click_resets_and_the_wheel_steps() {
    let (mut d, reg, mut v) = setup();
    let p = v.swing_field_center();
    let a = feed(&mut v, &d, &reg, &[wheel(p, 0.0, 3.0)]);
    apply_all(&mut d, &reg, &a);
    assert_eq!(clip_of(d.doc()).grid.unwrap().swing, 3);
    let dbl = down_with(p, atomik_ui::PointerButton::Primary, 2);
    let a = feed(&mut v, &d, &reg, &[dbl, up(p)]);
    apply_all(&mut d, &reg, &a);
    assert_eq!(clip_of(d.doc()).grid.unwrap().swing, 0);
}

#[test]
fn escape_during_a_swing_drag_restores_it() {
    let (mut d, reg, mut v) = setup();
    let p = v.swing_field_center();
    let a = feed(&mut v, &d, &reg, &[down(p), mv(p + Vec2::new(20.0, 0.0))]);
    apply_all(&mut d, &reg, &a);
    assert_eq!(clip_of(d.doc()).grid.unwrap().swing, 20);
    let a = feed(&mut v, &d, &reg, &[press(Key::Escape, Modifiers::NONE)]);
    apply_all(&mut d, &reg, &a);
    assert_eq!(clip_of(d.doc()).grid.unwrap().swing, 0);
}

#[test]
fn pressing_a_row_header_auditions_the_pad_until_release() {
    let (d, reg, mut v) = setup();
    let p = v.header_center(2);
    let a = feed(&mut v, &d, &reg, &[down(p)]);
    assert_eq!(auditions(&a), vec![(37, 100, true)]);
    assert!(commands(&a).is_empty());
    let a = feed(&mut v, &d, &reg, &[up(p)]);
    assert_eq!(auditions(&a), vec![(37, 100, false)]);
    let p = v.header_center(16);
    let a = click(&mut v, &d, &reg, p);
    assert_eq!(auditions(&a), vec![(51, 100, true), (51, 100, false)]);
}

#[test]
fn files_over_a_row_header_load_onto_that_pad_elsewhere_fill_empty_pads() {
    let (d, reg, mut v) = setup();
    let paths: Vec<PathBuf> = vec!["a.wav".into(), "b.wav".into()];
    let on_header = v.header_center(5);
    let hover = |pos| UiEvent::FileHover {
        pos,
        paths: paths.clone(),
    };
    let drop = |pos| UiEvent::FileDrop {
        pos,
        paths: paths.clone(),
    };
    let _ = feed(&mut v, &d, &reg, &[hover(on_header)]);
    assert_eq!(v.drop_hover(), Some(Some(pad_key(5))));
    let a = feed(&mut v, &d, &reg, &[drop(on_header)]);
    assert_eq!(
        a,
        vec![UiAction::ImportSamples {
            paths: paths.clone(),
            device: key("drums"),
            at_key: Some(40),
        }]
    );
    assert_eq!(v.drop_hover(), None);
    let on_grid = v.cell_center(3, 3);
    let _ = feed(&mut v, &d, &reg, &[hover(on_grid)]);
    assert_eq!(v.drop_hover(), Some(None));
    let a = feed(&mut v, &d, &reg, &[drop(on_grid)]);
    assert_eq!(
        a,
        vec![UiAction::ImportSamples {
            paths: paths.clone(),
            device: key("drums"),
            at_key: None
        }]
    );
    let _ = feed(
        &mut v,
        &d,
        &reg,
        &[hover(on_header), UiEvent::FileHoverCancel],
    );
    assert_eq!(v.drop_hover(), None);
    let _ = feed(&mut v, &d, &reg, &[hover(Point::new(2000.0, 10.0))]);
    assert_eq!(v.drop_hover(), None, "outside the view");
    let none = UiEvent::FileDrop {
        pos: on_header,
        paths: Vec::new(),
    };
    assert!(
        feed(&mut v, &d, &reg, &[none]).is_empty(),
        "no files, no import"
    );
    // The hover indicator is painted.
    let _ = feed(&mut v, &d, &reg, &[hover(on_header)]);
    let t = texts(&mut v, &d, &reg);
    assert!(t.iter().any(|x| x == "Drop 2 files on pad 5"), "{t:?}");
}

#[test]
fn without_a_clip_the_editor_offers_a_new_pattern() {
    let (d, reg, mut v) = setup_with(drum_doc_with(None), None);
    assert!(v.is_present());
    assert_eq!(v.clip(), None);
    let t = texts(&mut v, &d, &reg);
    for s in ["No pattern", "NEW PATTERN"] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    // No grid controls and no cells in the empty state.
    let a = ck!(v, d, reg, v.cell_center(1, 0));
    assert!(a.is_empty(), "{a:?}");
    let a = ck!(v, d, reg, v.new_pattern_button_center());
    assert_eq!(
        a,
        vec![UiAction::CreateStepClip {
            device: key("drums")
        }]
    );
    // Drops still go to the pane (the app fills empty pads).
    let paths: Vec<PathBuf> = vec!["a.wav".into()];
    let ev = UiEvent::FileDrop {
        pos: Point::new(300.0, 300.0),
        paths: paths.clone(),
    };
    let a = feed(&mut v, &d, &reg, &[ev]);
    assert_eq!(
        a,
        vec![UiAction::ImportSamples {
            paths,
            device: key("drums"),
            at_key: None
        }]
    );
}

#[test]
fn open_close_and_presence() {
    let (d, reg, mut v) = setup();
    let a = ck!(v, d, reg, v.close_button_center());
    assert_eq!(a, vec![UiAction::CloseStepEditor]);
    let a = feed(&mut v, &d, &reg, &[press(Key::Escape, Modifiers::NONE)]);
    assert_eq!(a, vec![UiAction::CloseStepEditor]);
    // A deleted clip or device makes the view absent.
    let mut doc = d.doc().clone();
    doc.song.tracks[0].lanes[0].clips.clear();
    v.sync(&doc, &reg);
    assert!(!v.is_present());
    let mut doc = drum_doc();
    doc.devices.retain(|x| x.key != key("drums"));
    doc.song.tracks.clear();
    v.sync(&doc, &reg);
    assert!(!v.is_present());
    // Reopening on the real document works; close() empties the view.
    v.open(key("drums"), Some(CLIP));
    v.sync(&drum_doc(), &reg);
    assert!(v.is_present());
    v.close();
    assert!(!v.is_present() && v.device().is_none());
    let a = v.handle(&down(Point::new(10.0, 10.0)), &drum_doc(), &reg);
    assert!(a.is_empty());
    // A device of another type is not a drum machine.
    let mut other = StepEditorView::new(atomik_ui::Theme::dark());
    other.open(key("synth"), None);
    other.sync(&reference_doc(), &reg);
    assert!(!other.is_present());
}

#[test]
fn undo_and_redo_shortcuts() {
    let (d, reg, mut v) = setup();
    let a = feed(&mut v, &d, &reg, &[press(Key::Char('z'), Modifiers::CTRL)]);
    assert_eq!(a, vec![UiAction::Undo]);
    let a = feed(&mut v, &d, &reg, &[press(Key::Char('y'), Modifiers::CTRL)]);
    assert_eq!(a, vec![UiAction::Redo]);
    let both = Modifiers {
        shift: true,
        ctrl: true,
        ..Modifiers::NONE
    };
    let a = feed(&mut v, &d, &reg, &[press(Key::Char('z'), both)]);
    assert_eq!(a, vec![UiAction::Redo]);
}

#[test]
fn resolve_clip_prefers_the_selection_then_the_playhead() {
    let mut doc = drum_doc();
    let mut second = beat_clip();
    second.id = ClipId(8);
    second.start = 4 * PPQ * 2;
    doc.song.tracks[0].lanes[0].clips.push(second);
    let dk = key("drums");
    let r = StepEditorView::resolve_clip;
    assert_eq!(
        r(&doc, &dk, Some(ClipId(8)), 0),
        Some(ClipId(8)),
        "selected wins"
    );
    assert_eq!(
        r(&doc, &dk, None, 100),
        Some(ClipId(7)),
        "under the playhead"
    );
    assert_eq!(r(&doc, &dk, None, 4 * PPQ * 2 + 5), Some(ClipId(8)));
    assert_eq!(r(&doc, &dk, None, 4 * PPQ * 3), None, "a gap");
    assert_eq!(
        r(&doc, &dk, Some(ClipId(99)), 100),
        Some(ClipId(7)),
        "unknown selection"
    );
    assert_eq!(r(&doc, &key("synth"), Some(ClipId(7)), 0), None, "no track");
}

#[test]
fn a_long_grid_scrolls_horizontally_and_a_scrolled_cell_still_hits() {
    let mut clip = beat_clip();
    clip.length = 64 * S16;
    clip.grid = Some(grid(StepResolution::Sixteenth, 64, 0));
    let (mut d, reg, mut v) = setup_with(drum_doc_with(Some(clip)), Some(CLIP));
    assert!(
        v.cell_rect(1, 63).x1 > v.bounds().x1 - 8.0,
        "64 steps overflow the pane"
    );
    let p = v.cell_center(1, 2);
    let _ = feed(&mut v, &d, &reg, &[wheel(p, -30.0, 0.0)]);
    assert!(
        v.cell_rect(1, 63).x1 <= v.bounds().x1,
        "scrolled to the end"
    );
    let a = ck!(v, d, reg, v.cell_center(1, 63));
    apply_all(&mut d, &reg, &a);
    resync(&mut v, &d, &reg);
    assert!(v.cell_velocity(1, 63).is_some());
}

// ------------------------------------------------------------------ proptest

#[derive(Debug, Clone)]
enum G {
    Click { pad: u8, step: u16 },
    Stroke { pad: u8, from: u16, to: u16 },
    Velocity { pad: u8, step: u16, dy: i16 },
    Header(u8),
    Res(u8),
    Steps(u8),
    Swing(i16),
    Level(u8),
    Escape { pad: u8, from: u16, to: u16 },
}

fn gestures() -> impl Strategy<Value = Vec<G>> {
    let g = prop_oneof![
        (1u8..=16, 0u16..16).prop_map(|(pad, step)| G::Click { pad, step }),
        (1u8..=16, 0u16..16, 0u16..16).prop_map(|(pad, from, to)| G::Stroke { pad, from, to }),
        (1u8..=16, 0u16..16, -120i16..120).prop_map(|(pad, step, dy)| G::Velocity {
            pad,
            step,
            dy
        }),
        (1u8..=16).prop_map(G::Header),
        (0u8..3).prop_map(G::Res),
        (0u8..4).prop_map(G::Steps),
        (-100i16..100).prop_map(G::Swing),
        (0u8..3).prop_map(G::Level),
        (1u8..=16, 0u16..16, 0u16..16).prop_map(|(pad, from, to)| G::Escape { pad, from, to }),
    ];
    proptest::collection::vec(g, 1..14)
}

fn run_gesture(
    v: &mut StepEditorView,
    d: &mut Document,
    reg: &Registry,
    g: &G,
    merges: &mut BTreeSet<u64>,
) {
    let steps = v.grid().steps;
    let cell = |v: &StepEditorView, pad: u8, step: u16| v.cell_center(pad, step.min(steps - 1));
    let actions = match g {
        G::Click { pad, step } => click(v, d, reg, cell(v, *pad, *step)),
        G::Stroke { pad, from, to } => {
            let (a, b) = (cell(v, *pad, *from), cell(v, *pad, *to));
            gesture(v, d, reg, &[a, Point::new((a.x + b.x) / 2.0, a.y), b])
        }
        G::Velocity { pad, step, dy } => {
            let p = cell(v, *pad, *step);
            let dy = f64::from(*dy);
            gesture(
                v,
                d,
                reg,
                &[p, p + Vec2::new(0.0, dy / 2.0), p + Vec2::new(0.0, dy)],
            )
        }
        G::Header(pad) => click(v, d, reg, v.header_center(*pad)),
        G::Res(i) => {
            let r = [
                StepResolution::Eighth,
                StepResolution::Sixteenth,
                StepResolution::ThirtySecond,
            ][usize::from(*i)];
            click(v, d, reg, v.resolution_button_center(r))
        }
        G::Steps(i) => click(
            v,
            d,
            reg,
            v.steps_button_center([16, 32, 48, 64][usize::from(*i)]),
        ),
        G::Swing(dx) => {
            let p = v.swing_field_center();
            let dx = f64::from(*dx);
            gesture(
                v,
                d,
                reg,
                &[p, p + Vec2::new(dx / 2.0, 0.0), p + Vec2::new(dx, 0.0)],
            )
        }
        G::Level(i) => click(
            v,
            d,
            reg,
            v.level_button_center(VelocityLevel::ALL[usize::from(*i)]),
        ),
        G::Escape { pad, from, to } => {
            let (a, b) = (cell(v, *pad, *from), cell(v, *pad, *to));
            let mut acts = feed(v, d, reg, &[down(a), mv(b)]);
            acts.extend(feed(v, d, reg, &[press(Key::Escape, Modifiers::NONE)]));
            acts.extend(feed(v, d, reg, &[up(b)]));
            acts
        }
    };
    // One merge id per drag gesture, fresh per gesture.
    let grid_merges = commands(&actions).into_iter().filter_map(|c| match c {
        Command::Song(SongCommand::SetClipGrid { merge, .. }) => Some(merge),
        _ => None,
    });
    let ids: Vec<u64> = note_lists(&actions)
        .iter()
        .map(|(_, m)| *m)
        .chain(grid_merges)
        .flatten()
        .collect();
    if !ids.is_empty() {
        assert!(
            ids.iter().all(|i| *i == ids[0]),
            "one id per gesture: {ids:?}"
        );
        assert!(merges.insert(ids[0]), "fresh id per gesture");
        assert!(ends(&actions) >= 1);
    }
    apply_all(d, reg, &actions);
    resync(v, d, reg);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn random_gestures_never_emit_a_command_the_model_rejects(gs in gestures()) {
        let reg = registry();
        let mut doc = drum_doc();
        // A note the grid never shows (key 60 is no pad).
        if let ClipContent::Notes(n) = &mut doc.song.tracks[0].lanes[0].clips[0].content {
            n.push(Note { tick: 5, length: 10, key: 60, velocity: 0.5 });
            n.sort_by_key(|n| (n.tick, n.key));
        }
        let mut v = editor(&doc, &reg, Some(CLIP));
        let mut d = Document::new(doc);
        let mut merges = BTreeSet::new();
        for g in &gs {
            run_gesture(&mut v, &mut d, &reg, g, &mut merges);
        }
        let notes = notes_of(&d);
        prop_assert!(notes.iter().filter(|n| n.key == 60).count() == 1, "off-grid notes survive");
        prop_assert!(notes.iter().all(|n| n.length > 0 && n.velocity > 0.0 && n.velocity <= 1.0));
        let clip = clip_of(d.doc());
        if let Some(g) = clip.grid {
            prop_assert!(g.validate().is_ok());
            prop_assert_eq!(clip.length, i64::from(g.steps) * g.resolution.ticks());
        }
    }
}
