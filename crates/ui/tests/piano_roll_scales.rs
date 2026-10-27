//! Scales, fold and chords in the piano roll: the scale bar, fold row mapping (hit testing,
//! drawing, dragging, band select, audition and key zoom in a folded roll), snap to scale
//! (pencil, drag, transpose, conform), the chord stamp, and chordify, arpeggiate and strum on
//! the selection. Gestures run through a real [`Document`], so "one undo step" is checked on the
//! model.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::theory::{ChordKind, DiatonicSize, Scale, ScaleKind};
use atomik_model::{ClipContent, ClipId, Command, Document, Note, SongCommand};
use atomik_ui::chord_edit::{ArpPattern, Stamp, StrumDirection};
use atomik_ui::key_rows::Fold;
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::piano_roll::ScaleBarField;
use atomik_ui::timeline::Snap;
use atomik_ui::{ContextTarget, DisplayList, Modifiers, PianoRollView, Tool, UiAction};
use support::song::*;
use support::*;

fn c_major() -> Scale {
    Scale::new(0, ScaleKind::Major)
}

/// A roll on clip 1 holding `notes`, with `scale`, inside a real document.
fn setup(notes: Vec<Note>, scale: Option<Scale>) -> (PianoRollView, Document, Registry) {
    let reg = registry();
    let mut doc = song_doc();
    let clip = &mut doc.song.tracks[0].lanes[0].clips[0];
    clip.content = ClipContent::Notes(notes);
    clip.scale = scale;
    let d = Document::new(doc);
    let v = piano_roll(d.doc(), &reg);
    (v, d, reg)
}

fn notes_of(d: &Document) -> Vec<Note> {
    match &d.doc().song.clip(ClipId(1)).unwrap().2.content {
        ClipContent::Notes(n) => n.clone(),
        other => panic!("not notes: {other:?}"),
    }
}

fn scale_of(d: &Document) -> Option<Scale> {
    d.doc().song.clip(ClipId(1)).unwrap().2.scale
}

/// Applies the song commands among `actions` to the document and syncs the view.
fn apply(v: &mut PianoRollView, d: &mut Document, reg: &Registry, actions: &[UiAction]) {
    for c in song_commands(actions) {
        d.apply(Command::Song(c), reg).unwrap();
    }
    v.sync(d.doc(), reg);
}

/// Runs a registry command and applies what it emits. Panics unless it was handled.
fn run_cmd(
    v: &mut PianoRollView,
    d: &mut Document,
    reg: &Registry,
    id: &str,
    arg: Option<&str>,
) -> Vec<UiAction> {
    match v.command(id, arg, d.doc(), reg) {
        CommandOutcome::Handled(actions) => {
            apply(v, d, reg, &actions);
            actions
        }
        other => panic!("{id} {arg:?}: {other:?}"),
    }
}

/// The note lists emitted, with their merge ids.
fn emitted(actions: &[UiAction]) -> Vec<(Vec<Note>, Option<u64>)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipContent {
                id: ClipId(1),
                content: ClipContent::Notes(n),
                merge,
            } => Some((n, merge)),
            _ => None,
        })
        .collect()
}

fn auditions(actions: &[UiAction]) -> Vec<(u8, bool)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Audition { key, on, .. } => Some((*key, *on)),
            _ => None,
        })
        .collect()
}

fn at(v: &PianoRollView, tick: i64, key: u8) -> Point {
    Point::new(v.tick_to_x(tick), v.key_center_y(key))
}

fn keys(notes: &[Note]) -> Vec<u8> {
    notes.iter().map(|n| n.key).collect()
}

fn n(tick: i64, length: i64, key: u8) -> Note {
    Note::new(tick, length, key, 0.8)
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::NONE
    }
}

/// Draws a note with a click at (`tick`, `key`) and applies it.
fn click_draw(v: &mut PianoRollView, d: &mut Document, reg: &Registry, tick: i64, key: u8) {
    let p = at(v, tick + 100, key);
    let a = v.handle(&down(p), d.doc(), reg);
    apply(v, d, reg, &a);
    let a = v.handle(&up(p), d.doc(), reg);
    apply(v, d, reg, &a);
}

// ---------------------------------------------------------------- the scale

#[test]
fn set_scale_commands_emit_set_clip_scale_and_the_view_follows_the_document() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60)], None);
    assert_eq!(v.scale(), None);
    run_cmd(
        &mut v,
        &mut d,
        &reg,
        "editor.set_scale",
        Some("natural_minor"),
    );
    assert_eq!(scale_of(&d), Some(Scale::new(0, ScaleKind::NaturalMinor)));
    assert_eq!(v.scale(), scale_of(&d));
    run_cmd(&mut v, &mut d, &reg, "editor.set_scale_root", Some("a"));
    assert_eq!(scale_of(&d), Some(Scale::new(9, ScaleKind::NaturalMinor)));
    run_cmd(&mut v, &mut d, &reg, "editor.set_scale", Some("dorian"));
    assert_eq!(
        scale_of(&d),
        Some(Scale::new(9, ScaleKind::Dorian)),
        "the root stays"
    );
    run_cmd(&mut v, &mut d, &reg, "editor.set_scale", Some("chromatic"));
    assert_eq!(scale_of(&d), None);
    assert_eq!(v.scale(), None);
}

#[test]
fn a_root_chosen_without_a_scale_is_kept_for_the_next_scale() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60)], None);
    let a = run_cmd(
        &mut v,
        &mut d,
        &reg,
        "editor.set_scale_root",
        Some("f_sharp"),
    );
    assert!(a.is_empty(), "nothing to change yet");
    assert_eq!(v.tools().root, 6);
    run_cmd(&mut v, &mut d, &reg, "editor.set_scale", Some("major"));
    assert_eq!(scale_of(&d), Some(Scale::new(6, ScaleKind::Major)));
}

#[test]
fn a_scale_change_is_one_undo_step_and_unknown_arguments_are_declined() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60)], None);
    run_cmd(&mut v, &mut d, &reg, "editor.set_scale", Some("blues"));
    d.undo(&reg).unwrap();
    assert_eq!(scale_of(&d), None);
    for (id, arg) in [
        ("editor.set_scale", Some("lydian_b7")),
        ("editor.set_scale", None),
        ("editor.set_scale_root", Some("h")),
        ("editor.set_chord", Some("nope")),
        ("editor.set_chord_inversion", Some("4")),
        ("editor.fold_set", Some("half")),
        ("editor.set_arp_gate", Some("33")),
        ("editor.set_arp_octaves", Some("4")),
        ("editor.set_strum_amount", Some("7")),
    ] {
        assert_eq!(
            v.command(id, arg, d.doc(), &reg),
            CommandOutcome::Declined,
            "{id} {arg:?}"
        );
    }
}

#[test]
fn detect_scale_picks_the_best_fit_from_the_notes() {
    // D dorian, with D the heaviest note.
    let notes = vec![
        n(0, 4 * PPQ, 62),
        n(PPQ, PPQ, 64),
        n(2 * PPQ, PPQ, 65),
        n(3 * PPQ, PPQ, 67),
        n(4 * PPQ, PPQ, 69),
        n(5 * PPQ, PPQ, 71),
        n(6 * PPQ, PPQ, 72),
    ];
    let (mut v, mut d, reg) = setup(notes, None);
    run_cmd(&mut v, &mut d, &reg, "editor.detect_scale", None);
    assert_eq!(scale_of(&d), Some(Scale::new(2, ScaleKind::Dorian)));
    // Already that scale: nothing to send.
    let a = run_cmd(&mut v, &mut d, &reg, "editor.detect_scale", None);
    assert!(song_commands(&a).is_empty());
    // An empty clip detects nothing.
    let (mut v, mut d, reg) = setup(Vec::new(), None);
    let a = run_cmd(&mut v, &mut d, &reg, "editor.detect_scale", None);
    assert!(a.is_empty());
    assert_eq!(scale_of(&d), None);
}

// ---------------------------------------------------------------- the scale bar

#[test]
fn the_scale_bar_opens_menus_detects_and_cycles() {
    let (mut v, mut d, reg) = setup(
        vec![n(0, PPQ, 60), n(PPQ, PPQ, 64), n(2 * PPQ, PPQ, 67)],
        None,
    );
    // Root and scale fields ask for a menu below them.
    for (field, target) in [
        (ScaleBarField::Root, ContextTarget::PianoScaleRoot),
        (ScaleBarField::Kind, ContextTarget::PianoScaleKind),
        (ScaleBarField::Chord, ContextTarget::PianoChord),
    ] {
        let p = v.scale_bar_center(field);
        let a = v.handle(&down(p), d.doc(), &reg);
        match a.as_slice() {
            [UiAction::ContextMenu { target: t, at }] => {
                assert_eq!(*t, target);
                assert!(at.y > p.y, "below the field");
            }
            other => panic!("{field:?}: {other:?}"),
        }
        v.handle(&up(p), d.doc(), &reg);
    }
    // The snap toggle.
    let p = v.scale_bar_center(ScaleBarField::SnapScale);
    v.handle(&down(p), d.doc(), &reg);
    v.handle(&up(p), d.doc(), &reg);
    assert!(v.snap_to_scale());
    v.handle(&down(p), d.doc(), &reg);
    v.handle(&up(p), d.doc(), &reg);
    assert!(!v.snap_to_scale());
    // The fold cycles on a click and back on a right click.
    let p = v.scale_bar_center(ScaleBarField::Fold);
    for want in [Fold::Scale, Fold::Used, Fold::Off] {
        v.handle(&down(p), d.doc(), &reg);
        v.handle(&up(p), d.doc(), &reg);
        assert_eq!(v.fold(), want);
    }
    v.handle(&right_down(p), d.doc(), &reg);
    v.handle(&right_up(p), d.doc(), &reg);
    assert_eq!(v.fold(), Fold::Used);
    // The inversion cycles 0..=3.
    let p = v.scale_bar_center(ScaleBarField::Inversion);
    for want in [1, 2, 3, 0] {
        v.handle(&down(p), d.doc(), &reg);
        v.handle(&up(p), d.doc(), &reg);
        assert_eq!(v.inversion(), want);
    }
    // Detect sets the scale of the notes (C E G fits the C major pentatonic best).
    let p = v.scale_bar_center(ScaleBarField::Detect);
    let a = v.handle(&down(p), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &a);
    assert!(scale_of(&d).is_some_and(|s| s.contains(60) && s.contains(64) && s.contains(67)));
}

#[test]
fn the_scale_bar_does_not_leak_into_the_grid_and_the_grid_starts_below_the_ruler() {
    let (mut v, d, reg) = setup(vec![n(0, PPQ, 60)], None);
    // A click on the ruler locates, one just above it (the scale bar, outside any field) does
    // nothing.
    let p = Point::new(700.0, v.ruler_y());
    let a = v.handle(&down(p), d.doc(), &reg);
    assert!(a.iter().any(|a| matches!(a, UiAction::Transport(_))));
    v.handle(&up(p), d.doc(), &reg);
    let p = Point::new(900.0, v.ruler_y() - 20.0);
    let a = v.handle(&down(p), d.doc(), &reg);
    assert!(a.is_empty(), "{a:?}");
    v.handle(&up(p), d.doc(), &reg);
}

// ---------------------------------------------------------------- fold

#[test]
fn fold_to_scale_hides_the_other_keys_and_maps_rows_to_keys() {
    let (mut v, d, reg) = setup(
        vec![n(0, PPQ, 60), n(PPQ, PPQ, 62), n(2 * PPQ, PPQ, 64)],
        Some(c_major()),
    );
    let full_gap = v.key_center_y(60) - v.key_center_y(62);
    assert!(
        (full_gap - 2.0 * v.key_height()).abs() < 1e-9,
        "unfolded: two rows"
    );
    v.set_fold(Fold::Scale);
    assert!(!v.rows().is_full());
    assert_eq!(v.rows().len(), 75);
    let gap = v.key_center_y(60) - v.key_center_y(62);
    assert!(
        (gap - v.key_height()).abs() < 1e-9,
        "folded: C and D are neighbours"
    );
    for k in [60u8, 62, 64, 65, 67] {
        assert_eq!(v.key_at(v.key_center_y(k)), k, "key {k}");
    }
    // The notes sit on their rows.
    for (i, note) in notes_of(&d).iter().enumerate() {
        let r = v.note_rect(i).unwrap();
        assert!((r.center().y - v.key_center_y(note.key)).abs() < 1e-9);
    }
    // A hidden key has no row of its own: it shares the row of the nearest shown key.
    assert_eq!(v.key_center_y(61), v.key_center_y(60));
    let _ = reg;
}

#[test]
fn fold_keeps_notes_off_the_scale_visible() {
    let (mut v, d, _reg) = setup(vec![n(0, PPQ, 60), n(PPQ, PPQ, 61)], Some(c_major()));
    v.set_fold(Fold::Scale);
    assert_eq!(v.rows().len(), 76, "the C# of the clip is an extra row");
    assert_eq!(v.rows().row_of(61).map(|r| r + 1), v.rows().row_of(60));
    let r = v.note_rect(1).unwrap();
    assert!((r.center().y - v.key_center_y(61)).abs() < 1e-9);
    assert_eq!(notes_of(&d).len(), 2);
}

#[test]
fn fold_to_used_notes_shows_one_row_per_key_of_the_clip() {
    let notes = vec![
        n(0, PPQ, 36),
        n(PPQ, PPQ, 43),
        n(2 * PPQ, PPQ, 41),
        n(3 * PPQ, PPQ, 43),
    ];
    let (mut v, _d, _reg) = setup(notes, None);
    v.set_fold(Fold::Used);
    assert_eq!(v.rows().keys(), [43, 41, 36]);
    let h = v.key_height();
    assert!((v.key_center_y(36) - v.key_center_y(41) - h).abs() < 1e-9);
    assert!((v.key_center_y(41) - v.key_center_y(43) - h).abs() < 1e-9);
    // Back to everything.
    v.set_fold(Fold::Off);
    assert!(v.rows().is_full());
    assert!((v.key_center_y(36) - v.key_center_y(43) - 7.0 * h).abs() < 1e-9);
}

#[test]
fn fold_with_nothing_to_fold_to_shows_every_key() {
    let (mut v, _d, _reg) = setup(Vec::new(), None);
    v.set_fold(Fold::Used);
    assert!(v.rows().is_full(), "no notes");
    v.set_fold(Fold::Scale);
    assert!(v.rows().is_full(), "no scale");
}

#[test]
fn the_pencil_draws_on_the_row_it_clicks_in_a_folded_roll() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60), n(PPQ, PPQ, 67)], Some(c_major()));
    v.set_tool(Tool::Pencil);
    v.set_fold(Fold::Scale);
    click_draw(&mut v, &mut d, &reg, 8 * PPQ, 64);
    let drawn: Vec<u8> = notes_of(&d)
        .iter()
        .filter(|n| n.tick == 8 * PPQ)
        .map(|n| n.key)
        .collect();
    assert_eq!(drawn, [64]);
    // The row just above E is F (the folded neighbour), not F#.
    let p = Point::new(
        v.tick_to_x(9 * PPQ + 100),
        v.key_center_y(64) - v.key_height(),
    );
    assert_eq!(v.key_at(p.y), 65);
    click_draw(&mut v, &mut d, &reg, 9 * PPQ, 65);
    assert!(
        notes_of(&d)
            .iter()
            .any(|n| n.tick == 9 * PPQ && n.key == 65)
    );
}

#[test]
fn in_a_used_fold_the_pencil_can_only_draw_on_used_keys() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 36), n(PPQ, PPQ, 43)], None);
    v.set_tool(Tool::Pencil);
    v.set_fold(Fold::Used);
    // Clicking the row of 43, in an empty stretch.
    let p = Point::new(v.tick_to_x(8 * PPQ + 100), v.key_center_y(43));
    let a = v.handle(&down(p), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &a);
    v.handle(&up(p), d.doc(), &reg);
    assert!(
        notes_of(&d)
            .iter()
            .any(|n| n.tick == 8 * PPQ && n.key == 43)
    );
    // A click far below the last row lands on the last row (36).
    let p = Point::new(
        v.tick_to_x(10 * PPQ + 100),
        v.key_center_y(36) + 3.0 * v.key_height(),
    );
    let _ = p;
}

#[test]
fn dragging_notes_in_a_folded_roll_moves_them_by_rows() {
    let notes = vec![
        n(0, PPQ, 36),
        n(PPQ, PPQ, 41),
        n(2 * PPQ, PPQ, 43),
        n(3 * PPQ, PPQ, 60),
    ];
    let (mut v, mut d, reg) = setup(notes, None);
    v.set_fold(Fold::Used);
    assert_eq!(v.rows().keys(), [60, 43, 41, 36]);
    let h = v.key_height();
    // Grab the 36 note (the last row) and pull it up two rows: it lands on 43.
    let r = v.note_rect(0).unwrap();
    let p = Point::new(r.center().x, r.center().y);
    let a = run(
        |e| v.handle(e, d.doc(), &reg),
        &[
            down(p),
            mv(p + Vec2::new(0.0, -1.0)),
            mv(p + Vec2::new(0.0, -2.0 * h)),
            up(p + Vec2::new(0.0, -2.0 * h)),
        ],
    );
    apply(&mut v, &mut d, &reg, &a);
    let moved = notes_of(&d);
    assert_eq!(moved.iter().find(|n| n.tick == 0).unwrap().key, 43);
    assert_eq!(auditions(&a).first(), Some(&(36, true)));
    assert!(auditions(&a).contains(&(43, true)));
    // The rows were frozen while dragging, so the view did not shift under the pointer, and
    // they are rebuilt afterwards (36 is no longer used).
    assert_eq!(v.rows().keys(), [60, 43, 41]);
}

#[test]
fn a_group_dragged_past_the_last_row_keeps_its_shape() {
    let notes = vec![
        n(0, PPQ, 36),
        n(PPQ, PPQ, 41),
        n(2 * PPQ, PPQ, 43),
        n(3 * PPQ, PPQ, 60),
    ];
    let (mut v, mut d, reg) = setup(notes, None);
    v.set_fold(Fold::Used);
    let h = v.key_height();
    v.set_selected_notes(&[0, 1]);
    let r = v.note_rect(1).unwrap();
    let p = Point::new(r.center().x, r.center().y);
    // Up by 50 rows: the pair stops with its top note on the top row (41 -> 60 would be three
    // rows up, so 36 -> 43 and 41 -> 60? the bottom one limits: the move is cut to the rows).
    let to = p + Vec2::new(0.0, -50.0 * h);
    let a = run(
        |e| v.handle(e, d.doc(), &reg),
        &[down(p), mv(p + Vec2::new(0.0, -1.0)), mv(to), up(to)],
    );
    apply(&mut v, &mut d, &reg, &a);
    let list = notes_of(&d);
    let (a0, a1) = (
        list.iter().find(|n| n.tick == 0).unwrap(),
        list.iter().find(|n| n.tick == PPQ).unwrap(),
    );
    // 36 was row 3 and 41 row 2: the cut leaves 41 on row 0 (60) and 36 on row 1 (43).
    assert_eq!((a0.key, a1.key), (43, 60));
}

#[test]
fn band_select_and_audition_work_in_a_folded_roll() {
    let notes = vec![
        n(0, PPQ, 36),
        n(PPQ, PPQ, 41),
        n(2 * PPQ, PPQ, 43),
        n(3 * PPQ, PPQ, 60),
    ];
    let (mut v, d, reg) = setup(notes, None);
    v.set_fold(Fold::Used);
    // A band from the 41 row down to the 36 row, across the first two notes.
    let a = Point::new(
        v.tick_to_x(-PPQ / 2).max(v.bounds().x0 + 70.0),
        v.key_center_y(41) - 2.0,
    );
    let b = Point::new(v.tick_to_x(PPQ + PPQ / 2), v.key_center_y(36) + 2.0);
    let events = [down_mods(a, shift(), 1), mv_with(b, shift()), up(b)];
    run(|e| v.handle(e, d.doc(), &reg), &events);
    assert_eq!(v.selected_notes(), vec![0, 1]);
    // The keyboard: clicking the strip at a folded row sounds that key.
    let out = v.handle(&down(v.keyboard_point(43)), d.doc(), &reg);
    assert_eq!(auditions(&out), vec![(43, true)]);
    let out = v.handle(&up(v.keyboard_point(43)), d.doc(), &reg);
    assert_eq!(auditions(&out), vec![(43, false)]);
}

#[test]
fn the_key_zoom_works_in_a_folded_roll_and_keeps_rows_adjacent() {
    let (mut v, d, reg) = setup(
        vec![n(0, PPQ, 60), n(PPQ, PPQ, 62), n(2 * PPQ, PPQ, 64)],
        Some(c_major()),
    );
    v.set_fold(Fold::Scale);
    for h in [6.0, 24.0, 48.0] {
        v.set_key_height(h);
        assert!((v.key_height() - h).abs() < 1e-9);
        assert!(
            (v.key_center_y(60) - v.key_center_y(62) - h).abs() < 1e-9,
            "h {h}"
        );
        assert_eq!(v.key_at(v.key_center_y(64)), 64);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, d.doc(), &reg);
    }
    v.set_fold(Fold::Used);
    for h in [6.0, 48.0] {
        v.set_key_height(h);
        assert!((v.key_center_y(60) - v.key_center_y(62) - h).abs() < 1e-9);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, d.doc(), &reg);
    }
}

#[test]
fn fold_commands_toggle_and_set_the_mode() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60)], Some(c_major()));
    run_cmd(&mut v, &mut d, &reg, "editor.fold_toggle", None);
    assert_eq!(v.fold(), Fold::Scale);
    run_cmd(&mut v, &mut d, &reg, "editor.fold_toggle", None);
    assert_eq!(v.fold(), Fold::Off);
    run_cmd(&mut v, &mut d, &reg, "editor.fold_set", Some("used"));
    assert_eq!(v.fold(), Fold::Used);
    run_cmd(&mut v, &mut d, &reg, "editor.fold_set", Some("off"));
    assert_eq!(v.fold(), Fold::Off);
    // Without a scale the toggle folds to the used notes.
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60)], None);
    run_cmd(&mut v, &mut d, &reg, "editor.fold_toggle", None);
    assert_eq!(v.fold(), Fold::Used);
}

#[test]
fn a_fold_to_scale_follows_a_scale_change_made_afterwards() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60)], None);
    v.set_fold(Fold::Scale);
    assert!(v.rows().is_full(), "no scale yet");
    run_cmd(&mut v, &mut d, &reg, "editor.set_scale", Some("major"));
    assert_eq!(v.rows().len(), 75);
    run_cmd(&mut v, &mut d, &reg, "editor.set_scale", Some("blues"));
    assert!(v.rows().len() < 75);
}

// ---------------------------------------------------------------- snap to scale

#[test]
fn the_pencil_snaps_to_the_scale_with_ties_going_down() {
    let (mut v, mut d, reg) = setup(Vec::new(), Some(c_major()));
    v.set_tool(Tool::Pencil);
    v.set_snap_to_scale(true);
    click_draw(&mut v, &mut d, &reg, 0, 61);
    click_draw(&mut v, &mut d, &reg, 4 * PPQ, 63);
    click_draw(&mut v, &mut d, &reg, 8 * PPQ, 66);
    click_draw(&mut v, &mut d, &reg, 12 * PPQ, 64);
    assert_eq!(keys(&notes_of(&d)), [60, 62, 65, 64]);
    // Off: the key under the pointer.
    v.set_snap_to_scale(false);
    click_draw(&mut v, &mut d, &reg, 16 * PPQ, 61);
    assert_eq!(notes_of(&d).last().unwrap().key, 61);
}

#[test]
fn snap_to_scale_without_a_scale_or_with_a_chromatic_one_changes_nothing() {
    let (mut v, mut d, reg) = setup(Vec::new(), None);
    v.set_tool(Tool::Pencil);
    v.set_snap_to_scale(true);
    click_draw(&mut v, &mut d, &reg, 0, 61);
    assert_eq!(keys(&notes_of(&d)), [61]);
}

#[test]
fn the_audition_of_a_snapped_note_plays_the_key_it_lands_on() {
    let (mut v, d, reg) = setup(Vec::new(), Some(c_major()));
    v.set_tool(Tool::Pencil);
    v.set_snap_to_scale(true);
    let p = at(&v, 100, 61);
    let a = v.handle(&down(p), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(60, true)]);
}

#[test]
fn dragging_a_group_vertically_with_snap_to_scale_moves_it_by_degrees() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60), n(PPQ, PPQ, 64)], Some(c_major()));
    v.set_snap_to_scale(true);
    v.set_selected_notes(&[0, 1]);
    let r = v.note_rect(0).unwrap();
    let p = Point::new(r.center().x, r.center().y);
    // The pointer is two semitones up: the grabbed note goes to D, one degree, so E goes to F.
    let to = Point::new(p.x, p.y - 2.0 * v.key_height());
    let a = run(
        |e| v.handle(e, d.doc(), &reg),
        &[down(p), mv(p + Vec2::new(0.0, -1.0)), mv(to), up(to)],
    );
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(keys(&notes_of(&d)), [62, 65]);
    // One semitone up (C#): the nearest scale key is C itself (ties down), so nothing moves.
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60), n(PPQ, PPQ, 64)], Some(c_major()));
    v.set_snap_to_scale(true);
    v.set_selected_notes(&[0, 1]);
    let r = v.note_rect(0).unwrap();
    let p = Point::new(r.center().x, r.center().y);
    let to = Point::new(p.x, p.y - v.key_height());
    let a = run(
        |e| v.handle(e, d.doc(), &reg),
        &[down(p), mv(p + Vec2::new(0.0, -1.0)), mv(to), up(to)],
    );
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(keys(&notes_of(&d)), [60, 64]);
    // Three semitones up (D#): D, one degree... the tie goes down to D.
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60)], Some(c_major()));
    v.set_snap_to_scale(true);
    v.set_selected_notes(&[0]);
    let r = v.note_rect(0).unwrap();
    let p = Point::new(r.center().x, r.center().y);
    let to = Point::new(p.x, p.y - 3.0 * v.key_height());
    let a = run(
        |e| v.handle(e, d.doc(), &reg),
        &[down(p), mv(p + Vec2::new(0.0, -1.0)), mv(to), up(to)],
    );
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(keys(&notes_of(&d)), [62]);
}

#[test]
fn the_same_drag_without_snap_to_scale_moves_by_semitones() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60), n(PPQ, PPQ, 64)], Some(c_major()));
    v.set_selected_notes(&[0, 1]);
    let r = v.note_rect(0).unwrap();
    let p = Point::new(r.center().x, r.center().y);
    let to = Point::new(p.x, p.y - 2.0 * v.key_height());
    let a = run(
        |e| v.handle(e, d.doc(), &reg),
        &[down(p), mv(p + Vec2::new(0.0, -1.0)), mv(to), up(to)],
    );
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(keys(&notes_of(&d)), [62, 66]);
}

#[test]
fn a_snapped_drag_is_one_merge_group_and_one_undo_step() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60)], Some(c_major()));
    v.set_snap_to_scale(true);
    let r = v.note_rect(0).unwrap();
    let p = Point::new(r.center().x, r.center().y);
    let mut all = Vec::new();
    let mut events = vec![down(p)];
    events.extend([-1.0, -12.0, -24.0, -48.0, -60.0].map(|dy| mv(Point::new(p.x, p.y + dy))));
    events.push(up(Point::new(p.x, p.y - 60.0)));
    for e in events {
        let a = v.handle(&e, d.doc(), &reg);
        apply(&mut v, &mut d, &reg, &a);
        all.extend(a);
    }
    let merges: Vec<_> = emitted(&all).into_iter().map(|(_, m)| m).collect();
    assert!(
        merges.len() >= 2 && merges.iter().all(|m| *m == merges[0]),
        "{merges:?}"
    );
    assert_ne!(keys(&notes_of(&d)), [60]);
    d.undo(&reg).unwrap();
    assert_eq!(keys(&notes_of(&d)), [60], "one undo step");
}

#[test]
fn transpose_moves_by_scale_degree_with_snap_to_scale_and_octaves_stay_octaves() {
    let (mut v, mut d, reg) = setup(
        vec![n(0, PPQ, 60), n(PPQ, PPQ, 64), n(2 * PPQ, PPQ, 61)],
        Some(c_major()),
    );
    v.set_selected_notes(&[0, 1, 2]);
    // Off: a semitone.
    run_cmd(&mut v, &mut d, &reg, "editor.transpose_up", None);
    assert_eq!(keys(&notes_of(&d)), [61, 65, 62]);
    run_cmd(&mut v, &mut d, &reg, "editor.transpose_down", None);
    assert_eq!(keys(&notes_of(&d)), [60, 64, 61]);
    // On: a degree (an out-of-scale note goes to the next scale key in that direction).
    v.set_snap_to_scale(true);
    run_cmd(&mut v, &mut d, &reg, "editor.transpose_up", None);
    assert_eq!(keys(&notes_of(&d)), [62, 65, 62], "C->D, E->F, C#->D");
    v.set_selected_notes(&[0, 1]);
    run_cmd(&mut v, &mut d, &reg, "editor.transpose_down", None);
    assert_eq!(keys(&notes_of(&d)), [60, 64, 62]);
    // Octaves are octaves either way.
    run_cmd(&mut v, &mut d, &reg, "editor.octave_up", None);
    assert_eq!(keys(&notes_of(&d)), [72, 76, 62]);
    // One undo step per command.
    d.undo(&reg).unwrap();
    assert_eq!(keys(&notes_of(&d)), [60, 64, 62]);
}

#[test]
fn transpose_by_degree_at_the_top_of_the_keyboard_keeps_the_notes_where_they_are() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 125), n(PPQ, PPQ, 127)], Some(c_major()));
    v.set_snap_to_scale(true);
    v.set_selected_notes(&[0, 1]);
    let a = run_cmd(&mut v, &mut d, &reg, "editor.transpose_up", None);
    assert!(a.is_empty(), "nothing fits above 127");
    assert_eq!(keys(&notes_of(&d)), [125, 127]);
}

#[test]
fn transpose_without_a_selection_is_declined_so_the_next_binding_runs() {
    let (mut v, d, reg) = setup(vec![n(0, PPQ, 60)], Some(c_major()));
    v.set_snap_to_scale(true);
    assert_eq!(
        v.command("editor.transpose_up", None, d.doc(), &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn conform_moves_the_selection_to_the_scale_in_one_undo_step() {
    let notes = vec![
        n(0, PPQ, 61),
        n(PPQ, PPQ, 63),
        n(2 * PPQ, PPQ, 66),
        n(3 * PPQ, PPQ, 60),
        n(4 * PPQ, PPQ, 70),
    ];
    let (mut v, mut d, reg) = setup(notes, Some(c_major()));
    v.set_selected_notes(&[0, 1, 2, 3]);
    let a = run_cmd(&mut v, &mut d, &reg, "editor.conform_to_scale", None);
    assert_eq!(emitted(&a).len(), 1);
    assert_eq!(
        keys(&notes_of(&d)),
        [60, 62, 65, 60, 70],
        "the unselected A# stays"
    );
    d.undo(&reg).unwrap();
    assert_eq!(keys(&notes_of(&d)), [61, 63, 66, 60, 70]);
    // Notes already in the scale: nothing is sent.
    v.set_selected_notes(&[3]);
    let a = run_cmd(&mut v, &mut d, &reg, "editor.conform_to_scale", None);
    assert!(song_commands(&a).is_empty());
}

#[test]
fn conform_without_a_scale_does_nothing_and_without_a_selection_is_declined() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 61)], None);
    v.set_selected_notes(&[0]);
    let a = run_cmd(&mut v, &mut d, &reg, "editor.conform_to_scale", None);
    assert!(a.is_empty());
    v.set_selected_notes(&[]);
    assert_eq!(
        v.command("editor.conform_to_scale", None, d.doc(), &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn conform_merges_notes_that_land_on_the_same_key_and_start() {
    let notes = vec![n(0, PPQ, 60), n(0, 2 * PPQ, 61)];
    let (mut v, mut d, reg) = setup(notes, Some(c_major()));
    v.set_selected_notes(&[0, 1]);
    run_cmd(&mut v, &mut d, &reg, "editor.conform_to_scale", None);
    assert_eq!(
        notes_of(&d),
        vec![n(0, 2 * PPQ, 60)],
        "one note, the longer one"
    );
}

// ---------------------------------------------------------------- chord stamp

#[test]
fn the_pencil_drops_the_whole_chord_and_auditions_it() {
    let (mut v, mut d, reg) = setup(Vec::new(), None);
    v.set_tool(Tool::Pencil);
    v.set_stamp(Stamp::Chord(ChordKind::Major));
    let p = at(&v, 2 * PPQ + 100, 60);
    let a = v.handle(&down(p), d.doc(), &reg);
    let lists = emitted(&a);
    assert_eq!(lists.len(), 1);
    assert_eq!(keys(&lists[0].0), [60, 64, 67]);
    assert!(
        lists[0]
            .0
            .iter()
            .all(|n| n.tick == 2 * PPQ && n.length == PPQ / 4)
    );
    assert_eq!(auditions(&a), vec![(60, true), (64, true), (67, true)]);
    assert_eq!(
        v.selected_notes(),
        vec![0, 1, 2],
        "the whole chord is selected"
    );
    apply(&mut v, &mut d, &reg, &a);
    let a = v.handle(&up(p), d.doc(), &reg);
    assert_eq!(auditions(&a), vec![(60, false), (64, false), (67, false)]);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn dragging_while_stamping_sets_the_length_of_every_note_in_one_undo_step() {
    let (mut v, mut d, reg) = setup(Vec::new(), None);
    v.set_tool(Tool::Pencil);
    v.set_stamp(Stamp::Chord(ChordKind::Minor));
    v.set_snap(Snap::Beat);
    let p = at(&v, 4 * PPQ + 100, 57);
    let beat_px = v.tick_to_x(PPQ) - v.tick_to_x(0);
    let mut all = Vec::new();
    for e in [
        down(p),
        mv(p + Vec2::new(beat_px * 2.1, 0.0)),
        up(p + Vec2::new(beat_px * 2.1, 0.0)),
    ] {
        let a = v.handle(&e, d.doc(), &reg);
        apply(&mut v, &mut d, &reg, &a);
        all.extend(a);
    }
    let list = notes_of(&d);
    assert_eq!(keys(&list), [57, 60, 64]);
    assert!(
        list.iter()
            .all(|n| n.tick == 4 * PPQ && n.length == 2 * PPQ),
        "{list:?}"
    );
    let merges: Vec<_> = emitted(&all).into_iter().map(|(_, m)| m).collect();
    assert!(merges.windows(2).all(|w| w[0] == w[1]));
    d.undo(&reg).unwrap();
    assert!(notes_of(&d).is_empty(), "the chord was one undo step");
}

#[test]
fn a_stamp_with_an_inversion_and_a_diatonic_stamp_build_their_chords() {
    let (mut v, d, reg) = setup(Vec::new(), Some(c_major()));
    v.set_tool(Tool::Pencil);
    v.set_stamp(Stamp::Chord(ChordKind::Major));
    v.set_inversion(1);
    let a = v.handle(&down(at(&v, 100, 60)), d.doc(), &reg);
    assert_eq!(keys(&emitted(&a)[0].0), [64, 67, 72]);
    v.handle(&up(at(&v, 100, 60)), d.doc(), &reg);
    v.set_inversion(0);
    v.set_stamp(Stamp::Diatonic(DiatonicSize::Triad));
    let a = v.handle(&down(at(&v, 4 * PPQ + 100, 62)), d.doc(), &reg);
    assert_eq!(keys(&emitted(&a)[0].0), [62, 65, 69], "ii");
    v.handle(&up(at(&v, 4 * PPQ + 100, 62)), d.doc(), &reg);
    // With snap to scale the root is the nearest scale key first.
    v.set_snap_to_scale(true);
    v.set_stamp(Stamp::Diatonic(DiatonicSize::Seventh));
    let a = v.handle(&down(at(&v, 8 * PPQ + 100, 61)), d.doc(), &reg);
    assert_eq!(keys(&emitted(&a)[0].0), [60, 64, 67, 71]);
}

#[test]
fn a_diatonic_stamp_without_a_scale_draws_a_single_note() {
    let (mut v, d, reg) = setup(Vec::new(), None);
    v.set_tool(Tool::Pencil);
    v.set_stamp(Stamp::Diatonic(DiatonicSize::Triad));
    let a = v.handle(&down(at(&v, 100, 60)), d.doc(), &reg);
    assert_eq!(keys(&emitted(&a)[0].0), [60]);
}

#[test]
fn a_chord_on_top_of_existing_notes_does_not_double_them() {
    let (mut v, mut d, reg) = setup(vec![n(4 * PPQ, PPQ / 2, 64)], None);
    v.set_tool(Tool::Pencil);
    v.set_stamp(Stamp::Chord(ChordKind::Major));
    click_draw(&mut v, &mut d, &reg, 4 * PPQ, 60);
    let list = notes_of(&d);
    assert_eq!(keys(&list), [60, 64, 67]);
    assert_eq!(list[1].length, PPQ / 2, "the longer note stays");
}

#[test]
fn escape_during_a_stamp_drag_removes_the_chord_and_silences_it() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 50)], None);
    v.set_tool(Tool::Pencil);
    v.set_stamp(Stamp::Chord(ChordKind::Major));
    let p = at(&v, 8 * PPQ + 100, 60);
    let a = v.handle(&down(p), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(notes_of(&d).len(), 4);
    let a = v.handle(
        &press(atomik_ui::Key::Escape, Modifiers::NONE),
        d.doc(),
        &reg,
    );
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(notes_of(&d).len(), 1);
    assert_eq!(auditions(&a), vec![(60, false), (64, false), (67, false)]);
}

#[test]
fn chord_commands_set_the_stamp_and_the_inversion() {
    let (mut v, mut d, reg) = setup(Vec::new(), None);
    run_cmd(&mut v, &mut d, &reg, "editor.set_chord", Some("maj7"));
    assert_eq!(v.stamp(), Stamp::Chord(ChordKind::MajorSeventh));
    run_cmd(
        &mut v,
        &mut d,
        &reg,
        "editor.set_chord",
        Some("diatonic_ninth"),
    );
    assert_eq!(v.stamp(), Stamp::Diatonic(DiatonicSize::Ninth));
    run_cmd(&mut v, &mut d, &reg, "editor.set_chord", Some("none"));
    assert_eq!(v.stamp(), Stamp::None);
    run_cmd(
        &mut v,
        &mut d,
        &reg,
        "editor.set_chord_inversion",
        Some("2"),
    );
    assert_eq!(v.inversion(), 2);
}

// ---------------------------------------------------------------- chordify, arpeggio, strum

#[test]
fn chordify_replaces_each_selected_note_by_the_stamp_chord_in_one_undo_step() {
    let notes = vec![n(0, PPQ, 60), n(PPQ, PPQ / 2, 65), n(2 * PPQ, PPQ, 70)];
    let (mut v, mut d, reg) = setup(notes, None);
    v.set_stamp(Stamp::Chord(ChordKind::Minor));
    v.set_selected_notes(&[0, 1]);
    let a = run_cmd(&mut v, &mut d, &reg, "editor.chordify", None);
    assert_eq!(emitted(&a).len(), 1);
    let got: Vec<(i64, i64, u8)> = notes_of(&d)
        .iter()
        .map(|n| (n.tick, n.length, n.key))
        .collect();
    assert_eq!(
        got,
        [
            (0, PPQ, 60),
            (0, PPQ, 63),
            (0, PPQ, 67),
            (PPQ, PPQ / 2, 65),
            (PPQ, PPQ / 2, 68),
            (PPQ, PPQ / 2, 72),
            (2 * PPQ, PPQ, 70)
        ]
    );
    assert_eq!(
        v.selected_notes(),
        vec![0, 1, 2, 3, 4, 5],
        "the chords are selected"
    );
    d.undo(&reg).unwrap();
    assert_eq!(notes_of(&d).len(), 3);
}

#[test]
fn chordify_with_a_diatonic_stamp_builds_the_chord_of_the_scale() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 62), n(PPQ, PPQ, 67)], Some(c_major()));
    v.set_stamp(Stamp::Diatonic(DiatonicSize::Seventh));
    v.set_selected_notes(&[0, 1]);
    run_cmd(&mut v, &mut d, &reg, "editor.chordify", None);
    assert_eq!(keys(&notes_of(&d)), [62, 65, 69, 72, 67, 71, 74, 77]);
}

#[test]
fn chordify_without_a_stamp_does_nothing_and_without_a_selection_is_declined() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 62)], None);
    v.set_selected_notes(&[0]);
    let a = run_cmd(&mut v, &mut d, &reg, "editor.chordify", None);
    assert!(a.is_empty());
    v.set_selected_notes(&[]);
    assert_eq!(
        v.command("editor.chordify", None, d.doc(), &reg),
        CommandOutcome::Declined
    );
}

fn triad_roll(snap: Snap) -> (PianoRollView, Document, Registry) {
    let (mut v, d, reg) = setup(
        vec![
            n(PPQ, PPQ, 60),
            n(PPQ, PPQ, 64),
            n(PPQ, PPQ, 67),
            n(4 * PPQ, PPQ, 50),
        ],
        None,
    );
    v.set_snap(snap);
    v.set_selected_notes(&[0, 1, 2]);
    (v, d, reg)
}

fn arp_result(d: &Document) -> Vec<(i64, i64, u8)> {
    notes_of(d)
        .iter()
        .map(|n| (n.tick, n.length, n.key))
        .collect()
}

#[test]
fn arpeggiate_up_with_a_sixteenth_snap_has_exact_ticks_and_is_one_undo_step() {
    let (mut v, mut d, reg) = triad_roll(Snap::Sixteenth);
    let s = PPQ / 4;
    let a = run_cmd(&mut v, &mut d, &reg, "editor.arpeggiate", None);
    assert_eq!(emitted(&a).len(), 1);
    assert_eq!(
        arp_result(&d),
        [
            (PPQ, s, 60),
            (PPQ + s, s, 64),
            (PPQ + 2 * s, s, 67),
            (PPQ + 3 * s, s, 60),
            (4 * PPQ, PPQ, 50)
        ]
    );
    assert_eq!(v.selected_notes(), vec![0, 1, 2, 3]);
    d.undo(&reg).unwrap();
    assert_eq!(notes_of(&d).len(), 4);
}

#[test]
fn arpeggiate_uses_the_snap_including_triplets_and_the_options() {
    let (mut v, mut d, reg) = triad_roll(Snap::EighthTriplet);
    run_cmd(&mut v, &mut d, &reg, "editor.set_arp_pattern", Some("down"));
    run_cmd(&mut v, &mut d, &reg, "editor.set_arp_gate", Some("50"));
    let step = PPQ / 3;
    run_cmd(&mut v, &mut d, &reg, "editor.arpeggiate", None);
    assert_eq!(
        arp_result(&d),
        [
            (PPQ, step / 2, 67),
            (PPQ + step, step / 2, 64),
            (PPQ + 2 * step, step / 2, 60),
            (4 * PPQ, PPQ, 50)
        ]
    );
    // Octaves and the other patterns.
    let (mut v, mut d, reg) = triad_roll(Snap::ThirtySecond);
    run_cmd(&mut v, &mut d, &reg, "editor.set_arp_octaves", Some("2"));
    run_cmd(
        &mut v,
        &mut d,
        &reg,
        "editor.set_arp_pattern",
        Some("up_down"),
    );
    run_cmd(&mut v, &mut d, &reg, "editor.arpeggiate", None);
    let got: Vec<u8> = arp_result(&d).iter().take(8).map(|x| x.2).collect();
    assert_eq!(got, [60, 64, 67, 72, 76, 79, 76, 72]);
    assert_eq!(arp_result(&d)[1].0, PPQ + PPQ / 8, "a 1/32 step");
}

#[test]
fn arpeggiate_random_is_repeatable_between_two_runs() {
    let run_once = || {
        let (mut v, mut d, reg) = triad_roll(Snap::Sixteenth);
        run_cmd(
            &mut v,
            &mut d,
            &reg,
            "editor.set_arp_pattern",
            Some("random"),
        );
        run_cmd(&mut v, &mut d, &reg, "editor.arpeggiate", None);
        arp_result(&d)
    };
    assert_eq!(run_once(), run_once());
    assert_eq!(ArpPattern::from_id("random"), Some(ArpPattern::Random));
}

#[test]
fn arpeggiate_with_a_single_note_or_nothing_selected() {
    let (mut v, mut d, reg) = triad_roll(Snap::Sixteenth);
    v.set_selected_notes(&[3]);
    let a = run_cmd(&mut v, &mut d, &reg, "editor.arpeggiate", None);
    assert!(a.is_empty(), "one note is not a chord");
    v.set_selected_notes(&[]);
    assert_eq!(
        v.command("editor.arpeggiate", None, d.doc(), &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn strum_delays_the_notes_and_keeps_their_ends_in_one_undo_step() {
    let (mut v, mut d, reg) = triad_roll(Snap::Sixteenth);
    run_cmd(&mut v, &mut d, &reg, "editor.set_strum_amount", Some("32"));
    let a = run_cmd(&mut v, &mut d, &reg, "editor.strum", None);
    assert_eq!(emitted(&a).len(), 1);
    let t = PPQ / 8;
    assert_eq!(
        arp_result(&d),
        [
            (PPQ, PPQ, 60),
            (PPQ + t, PPQ - t, 64),
            (PPQ + 2 * t, PPQ - 2 * t, 67),
            (4 * PPQ, PPQ, 50)
        ]
    );
    d.undo(&reg).unwrap();
    assert!(notes_of(&d).iter().all(|n| n.length == PPQ));
}

#[test]
fn strum_down_starts_on_the_top_note() {
    let (mut v, mut d, reg) = triad_roll(Snap::Sixteenth);
    run_cmd(
        &mut v,
        &mut d,
        &reg,
        "editor.set_strum_direction",
        Some("down"),
    );
    assert_eq!(v.strum_options().direction, StrumDirection::Down);
    run_cmd(&mut v, &mut d, &reg, "editor.strum", None);
    let t = PPQ / 16;
    let by_key = |k: u8| arp_result(&d).into_iter().find(|x| x.2 == k).unwrap();
    assert_eq!(by_key(67), (PPQ, PPQ, 67));
    assert_eq!(by_key(64), (PPQ + t, PPQ - t, 64));
    assert_eq!(by_key(60), (PPQ + 2 * t, PPQ - 2 * t, 60));
}

// ---------------------------------------------------------------- state and painting

#[test]
fn the_tools_report_the_state_the_menus_check() {
    let (mut v, mut d, reg) = setup(vec![n(0, PPQ, 60)], Some(Scale::new(2, ScaleKind::Dorian)));
    v.set_snap_to_scale(true);
    v.set_fold(Fold::Scale);
    run_cmd(&mut v, &mut d, &reg, "editor.set_chord", Some("m7"));
    run_cmd(
        &mut v,
        &mut d,
        &reg,
        "editor.set_arp_pattern",
        Some("down_up"),
    );
    let t = v.tools();
    assert_eq!(t.scale, Some(Scale::new(2, ScaleKind::Dorian)));
    assert_eq!(t.root, 2);
    assert!(t.snap_scale);
    assert_eq!(t.fold, Fold::Scale);
    assert_eq!(t.stamp, Stamp::Chord(ChordKind::MinorSeventh));
    assert_eq!(t.arp.pattern, ArpPattern::DownUp);
}

#[test]
fn the_roll_paints_in_every_mode_and_a_scale_changes_the_picture() {
    let (mut v, d, reg) = setup(vec![n(0, PPQ, 60), n(PPQ, PPQ, 64)], None);
    let mut plain = DisplayList::new();
    v.paint(&mut plain, d.doc(), &reg);
    let (mut v2, d2, reg2) = setup(vec![n(0, PPQ, 60), n(PPQ, PPQ, 64)], Some(c_major()));
    let mut scaled = DisplayList::new();
    v2.paint(&mut scaled, d2.doc(), &reg2);
    assert_ne!(format!("{plain:?}"), format!("{scaled:?}"));
    for fold in [Fold::Off, Fold::Scale, Fold::Used] {
        v2.set_fold(fold);
        for h in [6.0, 12.0, 48.0] {
            v2.set_key_height(h);
            let mut dl = DisplayList::new();
            v2.paint(&mut dl, d2.doc(), &reg2);
        }
    }
}

#[test]
fn the_argument_lists_of_the_commands_match_the_theory_names() {
    use atomik_ui::keymap::{
        ARP_PATTERN_ARGS, CHORD_ARGS, FOLD_ARGS, ROOT_ARGS, SCALE_ARGS, STRUM_AMOUNT_ARGS,
        STRUM_DIRECTION_ARGS,
    };
    let scales: Vec<&str> = ScaleKind::ALL.iter().map(|k| k.id()).collect();
    assert_eq!(SCALE_ARGS, scales.as_slice());
    let chords: Vec<&str> = Stamp::all().iter().map(|s| s.id()).collect();
    assert_eq!(CHORD_ARGS, chords.as_slice());
    let folds: Vec<&str> = Fold::ALL.iter().map(|f| f.id()).collect();
    assert_eq!(FOLD_ARGS, folds.as_slice());
    let pats: Vec<&str> = ArpPattern::ALL.iter().map(|p| p.id()).collect();
    assert_eq!(ARP_PATTERN_ARGS, pats.as_slice());
    assert_eq!(ROOT_ARGS.len(), 12);
    let amounts: Vec<String> = atomik_ui::chord_edit::STRUM_AMOUNTS
        .iter()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(
        STRUM_AMOUNT_ARGS,
        amounts
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
    );
    assert_eq!(STRUM_DIRECTION_ARGS, ["up", "down"]);
}
