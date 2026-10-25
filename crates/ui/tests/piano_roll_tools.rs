//! The piano roll's tool gating and its multi-note gestures: only the pencil draws, the select
//! tool rubber-bands from the empty grid, a selected group moves and resizes together (both
//! edges) in one undo step, and the cursor hint follows the hovered part and the drag.

mod support;

use atomik_model::{ClipContent, ClipId, Note, SongCommand};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{CursorHint, Modifiers, PianoRollView, Tool, UiAction, UiEvent};
use support::song::*;
use support::*;

fn setup() -> (
    PianoRollView,
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = piano_roll(&doc, &reg);
    v.set_snap(Snap::Beat);
    (v, doc, reg)
}

fn lists(actions: &[UiAction]) -> Vec<(Vec<Note>, Option<u64>)> {
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

fn beat_px(v: &PianoRollView) -> f64 {
    v.tick_to_x(PPQ) - v.tick_to_x(0)
}

fn end_merges(actions: &[UiAction]) -> usize {
    actions.iter().filter(|a| **a == UiAction::EndMerge).count()
}

/// A point on the empty grid, above the bass notes.
fn empty_point(v: &PianoRollView) -> Point {
    Point::new(v.tick_to_x(2 * PPQ), v.key_center_y(55))
}

#[test]
fn select_never_draws_a_click_or_a_drag_on_the_empty_grid() {
    let (mut v, doc, reg) = setup();
    assert_eq!(v.tool(), Tool::Select);
    v.set_selected_notes(&[0, 1]);
    let p = empty_point(&v);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert!(lists(&a).is_empty(), "a click draws nothing");
    assert!(
        v.selected_notes().is_empty(),
        "a click clears the selection"
    );
    let to = p + Vec2::new(beat_px(&v) * 2.0, 0.0);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), mv(to), up(p)]);
    assert!(lists(&a).is_empty(), "a drag draws nothing");
    assert_eq!(end_merges(&a), 0);
    // A double click (the second press) draws nothing either.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down_mods(p, Modifiers::NONE, 2), up(p)],
    );
    assert!(lists(&a).is_empty());
    // The pencil still draws.
    v.set_tool(Tool::Pencil);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert_eq!(lists(&a).len(), 1);
    assert_eq!(lists(&a)[0].0.len(), 17);
}

#[test]
fn a_plain_band_replaces_the_selection_and_shift_or_ctrl_adds() {
    let (mut v, doc, reg) = setup();
    v.set_selected_notes(&[15]);
    let a0 = Point::new(v.tick_to_x(0) + 1.0, v.key_center_y(52));
    let a1 = Point::new(v.tick_to_x(PPQ) + 20.0, v.key_center_y(34));
    let a = run(|e| v.handle(e, &doc, &reg), &[down(a0), mv(a1), up(a1)]);
    assert!(lists(&a).is_empty());
    assert_eq!(v.selected_notes(), vec![0, 1], "the old selection is gone");
    // The add modifier is Shift or the platform's Cmd. Literal Ctrl is the alternate tool on
    // macOS (it is not an add there), so only the other systems exercise it.
    let meta = Modifiers {
        meta: true,
        ..Modifiers::NONE
    };
    let mut adders = vec![Modifiers::SHIFT, meta];
    if !cfg!(target_os = "macos") {
        adders.push(Modifiers::CTRL);
    }
    for m in adders {
        v.set_selected_notes(&[15]);
        run(
            |e| v.handle(e, &doc, &reg),
            &[down_mods(a0, m, 1), mv_with(a1, m), up(a1)],
        );
        assert_eq!(v.selected_notes(), vec![0, 1, 15]);
    }
}

#[test]
fn dragging_a_selected_note_moves_the_whole_selection_in_one_step() {
    let (mut v, doc, reg) = setup();
    v.set_selected_notes(&[0, 1, 2]);
    let r = v.note_rect(1).unwrap();
    let p = r.center();
    let to = Point::new(
        p.x + 2.0 * beat_px(&v),
        v.key_center_y(bass_line()[1].key + 2),
    );
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(1.0, 0.0)), mv(to), up(to)],
    );
    let l = lists(&a);
    let notes = &l.last().unwrap().0;
    let base = bass_line();
    for (i, o) in base.iter().enumerate().take(3) {
        let moved = Note {
            tick: o.tick + 2 * PPQ,
            key: o.key + 2,
            ..*o
        };
        assert!(notes.contains(&moved), "note {i} moved by the same delta");
    }
    assert!(notes.contains(&base[5]), "the others stay");
    assert_eq!(v.selected_notes().len(), 3);
    assert!(l.iter().all(|(_, m)| *m == l[0].1), "one merge group");
    assert_eq!(end_merges(&a), 1);
}

#[test]
fn dragging_an_unselected_note_selects_it_alone_and_moves_only_it() {
    let (mut v, doc, reg) = setup();
    v.set_selected_notes(&[0, 1]);
    let p = v.note_rect(5).unwrap().center();
    let to = p + Vec2::new(beat_px(&v), 0.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(1.0, 0.0)), mv(to), up(to)],
    );
    assert_eq!(v.selected_notes().len(), 1);
    let notes = lists(&a).pop().unwrap().0;
    let base = bass_line();
    assert!(notes.contains(&base[0]) && notes.contains(&base[1]));
    assert!(notes.contains(&Note {
        tick: base[5].tick + PPQ,
        ..base[5]
    }));
}

#[test]
fn the_right_edge_resizes_every_selected_note_by_the_same_delta() {
    let (mut v, doc, reg) = setup();
    v.set_selected_notes(&[0, 3]);
    let r = v.note_rect(0).unwrap();
    let p = Point::new(r.x1 - 2.0, r.center().y);
    let to = p + Vec2::new(beat_px(&v), 0.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p + Vec2::new(1.0, 0.0)), mv(to), up(to)],
    );
    let notes = lists(&a).pop().unwrap().0;
    let base = bass_line();
    // The grabbed note snaps to two beats; the other one changes by the same delta.
    assert_eq!(notes[0].length, 2 * PPQ);
    assert_eq!(notes[3].length, base[3].length + 2 * PPQ - base[0].length);
    assert_eq!(notes[1].length, base[1].length, "unselected notes stay");
}

#[test]
fn the_left_edge_resizes_from_the_start_with_the_ends_fixed_for_the_whole_selection() {
    let (mut v, doc, reg) = setup();
    v.set_selected_notes(&[2, 3]);
    let base = bass_line();
    let r = v.note_rect(2).unwrap();
    let p = Point::new(r.x0 + 1.0, r.center().y);
    // One beat to the left: both starts move a beat earlier, the ends stay.
    let to = p - Vec2::new(beat_px(&v), 0.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(p - Vec2::new(1.0, 0.0)), mv(to), up(to)],
    );
    let notes = lists(&a).pop().unwrap().0;
    for i in [2usize, 3] {
        let o = base[i];
        let n = notes
            .iter()
            .find(|n| n.key == o.key && n.tick + n.length == o.tick + o.length)
            .unwrap_or_else(|| panic!("note {i} keeps its end: {notes:?}"));
        assert_eq!(n.tick, o.tick - PPQ);
    }
    assert_eq!(end_merges(&a), 1);
    // Dragging far to the right is limited: every note keeps a length.
    let bp = beat_px(&v);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(p),
            mv(p + Vec2::new(1.0, 0.0)),
            mv(p + Vec2::new(bp * 6.0, 0.0)),
            up(p),
        ],
    );
    let notes = lists(&a).pop().unwrap().0;
    assert!(notes.iter().all(|n| n.length > 0 && n.tick >= 0));
}

#[test]
fn the_cursor_hint_follows_the_hovered_part_the_tool_and_the_drag() {
    let (mut v, doc, reg) = setup();
    let r = v.note_rect(0).unwrap();
    let left = Point::new(r.x0 + 1.0, r.center().y);
    let body = r.center();
    let right = Point::new(r.x1 - 2.0, r.center().y);
    let empty = empty_point(&v);
    let hover = |v: &mut PianoRollView, p: Point| {
        v.handle(&mv(p), &doc, &reg);
        v.cursor()
    };
    assert_eq!(hover(&mut v, empty), CursorHint::Default);
    assert_eq!(hover(&mut v, left), CursorHint::ResizeHorizontal);
    assert_eq!(hover(&mut v, right), CursorHint::ResizeHorizontal);
    assert_eq!(hover(&mut v, body), CursorHint::Move);
    v.set_tool(Tool::Pencil);
    assert_eq!(hover(&mut v, empty), CursorHint::Crosshair);
    assert_eq!(hover(&mut v, right), CursorHint::Default);
    v.set_tool(Tool::Select);
    // During a drag the cursor keeps the gesture's shape wherever the pointer goes.
    v.handle(&down(right), &doc, &reg);
    v.handle(&mv(right + Vec2::new(30.0, 40.0)), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::ResizeHorizontal);
    v.handle(&up(right), &doc, &reg);
    v.handle(&down(body), &doc, &reg);
    v.handle(&mv(body + Vec2::new(30.0, 40.0)), &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Move);
    v.handle(&up(body), &doc, &reg);
    v.handle(&UiEvent::PointerLeave, &doc, &reg);
    assert_eq!(v.cursor(), CursorHint::Default);
}
