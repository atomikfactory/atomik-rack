//! The expression strip of the piano roll: the strip beside the edited note with its tabs and
//! buttons, and editing the curves by hand (add, move, delete, rubber band, freehand, apply to
//! several notes). Gestures run through a real [`Document`], so "one undo step" is checked on
//! the model.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipId, Command, Document, ExpressionKind, ExpressionPoint, MidiData, NoteExpression,
    SongCommand,
};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::viewport::Tool;
use atomik_ui::{DisplayList, Key, Modifiers, PianoRollView, UiAction};
use support::song::*;
use support::*;

fn pt(tick: i64, value: f32) -> ExpressionPoint {
    ExpressionPoint::new(tick, value)
}

/// The tick, key and length of the first note of clip 1.
fn first_note(d: &Document) -> (i64, u8, i64) {
    let c = d.doc().song.clip(ClipId(1)).unwrap().2.clone();
    let atomik_model::ClipContent::Notes(notes) = &c.content else {
        panic!("a note clip");
    };
    (notes[0].tick, notes[0].key, notes[0].length)
}

/// The first note of clip 1 gets curves; returns the roll with that note selected.
fn setup() -> (PianoRollView, Document, Registry, i64, u8) {
    let reg = registry();
    let mut doc = song_doc();
    let clip = &mut doc.song_mut().tracks[0].lanes[0].clips[0];
    let atomik_model::ClipContent::Notes(notes) = &clip.content else {
        panic!("a note clip");
    };
    let (tick, key) = (notes[0].tick, notes[0].key);
    let mut e = NoteExpression::new(tick, key);
    e.pitch = vec![pt(0, 0.0), pt(PPQ / 8, 1.5), pt(PPQ / 4, -2.0)];
    e.pressure = vec![pt(0, 0.2), pt(PPQ / 4, 0.9)];
    e.timbre = vec![pt(0, 0.5)];
    clip.midi = MidiData {
        expression: vec![e],
        ..Default::default()
    }
    .into();
    let d = Document::new(doc);
    let mut v = piano_roll(d.doc(), &reg);
    v.set_selected_notes(&[0]);
    (v, d, reg, tick, key)
}

/// The same with the first note's curves empty (no entry at all).
fn setup_plain() -> (PianoRollView, Document, Registry, i64, u8) {
    let (mut v, mut d, reg, tick, key) = setup();
    let midi = MidiData::default();
    d.apply(
        Command::Song(SongCommand::SetClipMidi {
            clip: ClipId(1),
            midi,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    (v, d, reg, tick, key)
}

fn midi_of(d: &Document) -> MidiData {
    (*d.doc().song.clip(ClipId(1)).unwrap().2.midi).clone()
}

fn curve(d: &Document, kind: ExpressionKind) -> Vec<ExpressionPoint> {
    let (tick, key, _) = first_note(d);
    midi_of(d)
        .expression_of(tick, key)
        .map(|e| e.curve(kind).to_vec())
        .unwrap_or_default()
}

/// Applies the commands among `actions` (ending merges on `EndMerge`) and syncs the view.
fn apply(v: &mut PianoRollView, d: &mut Document, reg: &Registry, actions: &[UiAction]) {
    for a in actions {
        match a {
            UiAction::Command(c) => {
                d.apply(c.clone(), reg)
                    .unwrap_or_else(|e| panic!("the model rejected {c:?}: {e}"));
            }
            UiAction::EndMerge => d.end_merge(),
            _ => {}
        }
    }
    v.sync(d.doc(), reg);
}

/// Feeds one event to the view and applies what comes back.
fn send(
    v: &mut PianoRollView,
    d: &mut Document,
    reg: &Registry,
    e: &atomik_ui::UiEvent,
) -> Vec<UiAction> {
    let a = v.handle(e, d.doc(), reg);
    apply(v, d, reg, &a);
    a
}

/// A press, moves through `points` and a release (with `mods` held on the moves).
fn drag(
    v: &mut PianoRollView,
    d: &mut Document,
    reg: &Registry,
    points: &[Point],
    mods: Modifiers,
) -> Vec<UiAction> {
    let mut all = Vec::new();
    let (first, rest) = points.split_first().unwrap();
    all.extend(send(v, d, reg, &down(*first)));
    for p in rest {
        all.extend(send(v, d, reg, &mv_with(*p, mods)));
    }
    all.extend(send(v, d, reg, &up(*points.last().unwrap())));
    all
}

fn click(v: &mut PianoRollView, d: &mut Document, reg: &Registry, p: Point) -> Vec<UiAction> {
    drag(v, d, reg, &[p], Modifiers::NONE)
}

/// The screen position of tick `tick` and `value` of the pitch plot of the strip.
fn plot_at(v: &PianoRollView, length: i64, kind: ExpressionKind, tick: i64, value: f32) -> Point {
    let plot = v.expression_plot().unwrap();
    let x = plot.x0 + tick as f64 / length as f64 * plot.width();
    let y = match kind {
        ExpressionKind::Pitch => {
            let half = f64::from(v.expression_range());
            (plot.y0 + plot.y1) / 2.0 - f64::from(value) / half * plot.height() / 2.0
        }
        _ => plot.y1 - f64::from(value) * plot.height(),
    };
    Point::new(x, y)
}

fn tab(v: &mut PianoRollView, d: &mut Document, reg: &Registry, kind: ExpressionKind) {
    let at = v.expression_tab_center(kind).unwrap();
    click(v, d, reg, at);
    assert_eq!(v.expression_kind(), kind);
}

fn ctrl() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    }
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::NONE
    }
}

// ----- the strip ----------------------------------------------------------------------------

#[test]
fn the_selected_note_shows_its_strip_beside_it() {
    let (v, _d, _reg, _, _) = setup();
    let (strip, clear) = v.expression_rects().expect("the note has expression");
    let note = v.note_rect(0).unwrap();
    assert!(
        strip.y0 >= note.y1 || strip.y1 <= note.y0,
        "under or over the note"
    );
    assert!(strip.x0 <= note.x0 + 1.0 && strip.width() >= 240.0 - 0.5);
    assert!(strip.contains(clear.center()));
    assert_eq!(v.expression_clear_center(), Some(clear.center()));
    assert!(strip.height() >= 70.0, "tall enough to edit in");
}

#[test]
fn a_note_without_expression_has_no_strip_until_it_is_opened() {
    let (mut v, mut d, reg, _, _) = setup_plain();
    assert!(v.expression_rects().is_none());
    let out = v.command("editor.edit_expression", None, d.doc(), &reg);
    assert!(matches!(out, CommandOutcome::Handled(_)));
    assert!(v.expression_open());
    assert!(v.expression_rects().is_some(), "now the strip shows");
    // With several notes selected it shows beside the first.
    v.set_selected_notes(&[1, 0, 2]);
    let (strip, _) = v.expression_rects().unwrap();
    let first = v.note_rect(0).unwrap();
    assert!(strip.x0 <= first.x0 + 1.0);
    v.set_selected_notes(&[]);
    assert!(v.expression_rects().is_none(), "no note, no strip");
    // The command again closes it; with nothing selected and it closed it declines.
    let out = v.command("editor.edit_expression", None, d.doc(), &reg);
    assert!(matches!(out, CommandOutcome::Handled(_)));
    assert!(!v.expression_open());
    let out = v.command("editor.edit_expression", None, d.doc(), &reg);
    assert!(matches!(out, CommandOutcome::Declined));
    let _ = &mut d;
}

#[test]
fn a_note_with_expression_among_several_shows_no_strip_unless_opened() {
    let (mut v, _d, _reg, _, _) = setup();
    v.set_selected_notes(&[0, 1]);
    assert!(v.expression_rects().is_none());
    v.set_selected_notes(&[1]);
    assert!(v.expression_rects().is_none());
    v.set_selected_notes(&[]);
    assert!(v.expression_rects().is_none());
}

#[test]
fn the_strip_is_painted_and_its_parts_have_tooltips_with_ids() {
    let (mut v, d, reg, _, _) = setup();
    let mut with = DisplayList::new();
    v.paint(&mut with, d.doc(), &reg);
    v.set_selected_notes(&[1]);
    let mut without = DisplayList::new();
    v.paint(&mut without, d.doc(), &reg);
    assert!(
        with.len() > without.len(),
        "the strip adds shapes: {} vs {}",
        with.len(),
        without.len()
    );
    v.set_selected_notes(&[0, 1]);
    v.command("editor.edit_expression", None, d.doc(), &reg);
    v.set_selected_notes(&[0]);
    let parts = [
        (v.expression_clear_center().unwrap(), "roll.expr_clear"),
        (
            v.expression_tab_center(ExpressionKind::Timbre).unwrap(),
            "roll.expr_tab",
        ),
        (v.expression_range_center().unwrap(), "roll.expr_range"),
        (
            plot_at(&v, first_note(&d).2, ExpressionKind::Pitch, 100, 20.0),
            "roll.expr_curve",
        ),
    ];
    for (at, id) in parts {
        let help = v
            .help_at(at)
            .unwrap_or_else(|| panic!("a tooltip for {id}"));
        assert_eq!(help.id.as_deref(), Some(id));
        assert!(help.text.len() <= 90, "{id}: {}", help.text.len());
    }
    v.set_selected_notes(&[0, 1]);
    let apply = v.expression_apply_center().expect("apply with two notes");
    let help = v.help_at(apply).unwrap();
    assert_eq!(help.id.as_deref(), Some("roll.expr_apply"));
    assert!(help.text.len() <= 90);
}

#[test]
fn the_clear_button_clears_the_notes_expression_as_one_undo_step() {
    let (mut v, mut d, reg, tick, key) = setup();
    let before = d.doc().clone();
    let at = v.expression_clear_center().unwrap();
    let a = v.handle(&down(at), d.doc(), &reg);
    let b = v.handle(&up(at), d.doc(), &reg);
    let sent: Vec<SongCommand> = song_commands(&[a, b].concat());
    assert_eq!(sent.len(), 1, "one command");
    assert!(matches!(
        sent[0],
        SongCommand::SetClipMidi { merge: None, .. }
    ));
    d.apply(Command::Song(sent[0].clone()), &reg).unwrap();
    assert!(midi_of(&d).expression_of(tick, key).is_none());
    v.sync(d.doc(), &reg);
    assert!(v.expression_rects().is_none(), "the strip is gone");
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "one undo step restores it");
}

#[test]
fn the_tabs_choose_the_curve_and_the_pitch_scale_steps_through_its_values() {
    let (mut v, mut d, reg, _, _) = setup();
    assert_eq!(v.expression_kind(), ExpressionKind::Pitch);
    assert!(v.expression_range_center().is_some());
    tab(&mut v, &mut d, &reg, ExpressionKind::Pressure);
    assert!(
        v.expression_range_center().is_none(),
        "only the pitch has a scale"
    );
    tab(&mut v, &mut d, &reg, ExpressionKind::Pitch);
    let scales: Vec<f32> = (0..5)
        .map(|_| {
            let at = v.expression_range_center().unwrap();
            let r = v.expression_range();
            click(&mut v, &mut d, &reg, at);
            r
        })
        .collect();
    assert_eq!(scales, [48.0, 24.0, 12.0, 2.0, 48.0]);
    // A right click goes back one step (the scale is 24 after the fifth click).
    let at = v.expression_range_center().unwrap();
    send(&mut v, &mut d, &reg, &right_down(at));
    send(&mut v, &mut d, &reg, &right_up(at));
    assert_eq!(v.expression_range(), 48.0, "24 and back one step");
}

#[test]
fn a_press_on_a_tab_changes_no_document() {
    let (mut v, mut d, reg, _, _) = setup();
    let before = d.doc().clone();
    let at = v.expression_tab_center(ExpressionKind::Timbre).unwrap();
    let a = drag(&mut v, &mut d, &reg, &[at], Modifiers::NONE);
    assert!(a.iter().all(|x| !matches!(x, UiAction::Command(_))));
    assert_eq!(d.doc(), &before);
    assert_eq!(v.selected_notes(), vec![0], "the selection stays");
}

// ----- editing the points -------------------------------------------------------------------

#[test]
fn a_click_on_empty_space_adds_a_point_as_one_undo_step() {
    let (mut v, mut d, reg, _, _) = setup();
    tab(&mut v, &mut d, &reg, ExpressionKind::Timbre);
    let (_, _, len) = first_note(&d);
    let before = d.doc().clone();
    let at = plot_at(&v, len, ExpressionKind::Timbre, len / 2, 0.75);
    let a = click(&mut v, &mut d, &reg, at);
    assert_eq!(song_commands(&a).len(), 1, "one command");
    let c = curve(&d, ExpressionKind::Timbre);
    assert_eq!(c.len(), 2);
    assert!((c[1].tick - len / 2).abs() <= len / 100, "{c:?}");
    assert!((c[1].value - 0.75).abs() < 0.03, "{c:?}");
    assert_eq!(
        v.expression_selection(),
        vec![1],
        "the new point is selected"
    );
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_click_adds_the_first_point_to_a_note_without_expression() {
    let (mut v, mut d, reg, tick, key) = setup_plain();
    v.command("editor.edit_expression", None, d.doc(), &reg);
    tab(&mut v, &mut d, &reg, ExpressionKind::Pressure);
    let (_, _, len) = first_note(&d);
    let at = plot_at(&v, len, ExpressionKind::Pressure, 0, 0.5);
    click(&mut v, &mut d, &reg, at);
    let e = midi_of(&d)
        .expression_of(tick, key)
        .cloned()
        .expect("an entry");
    assert_eq!(e.pressure.len(), 1);
    assert!(e.pitch.is_empty() && e.timbre.is_empty());
    d.undo().unwrap();
    assert!(
        midi_of(&d).expression.is_empty(),
        "one undo step removes it"
    );
}

#[test]
fn dragging_a_point_moves_it_inside_the_note_and_the_range_as_one_undo_step() {
    let (mut v, mut d, reg, _, _) = setup();
    tab(&mut v, &mut d, &reg, ExpressionKind::Pressure);
    let (_, _, len) = first_note(&d);
    let before = d.doc().clone();
    let start = v.expression_point_center(1).unwrap();
    let plot = v.expression_plot().unwrap();
    // Far past the right edge and above the top: held inside both.
    let a = drag(
        &mut v,
        &mut d,
        &reg,
        &[
            start,
            Point::new(start.x + 10.0, start.y - 5.0),
            Point::new(plot.x1 + 300.0, plot.y0 - 200.0),
        ],
        Modifiers::NONE,
    );
    let merges: Vec<Option<u64>> = song_commands(&a)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipMidi { merge, .. } => Some(merge),
            _ => None,
        })
        .collect();
    assert!(merges.len() >= 2 && merges.iter().all(|m| m.is_some()));
    assert!(merges.windows(2).all(|w| w[0] == w[1]), "one merge id");
    assert!(a.iter().any(|x| matches!(x, UiAction::EndMerge)));
    let c = curve(&d, ExpressionKind::Pressure);
    assert_eq!(c.len(), 2);
    assert_eq!(c[1].tick, len, "time stays inside the note");
    assert!((c[1].value - 1.0).abs() < 1e-6, "value stays in 0..=1");
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "one undo step");
}

#[test]
fn shift_drags_a_point_finely_and_control_snaps_a_pitch_to_semitones() {
    // A plain drag of 20 px up moves the pitch point by some amount; with Shift by a tenth of it.
    let moved = |mods: Modifiers| {
        let (mut v, mut d, reg, _, _) = setup();
        let start = v.expression_point_center(1).unwrap();
        let end = Point::new(start.x, start.y - 20.0);
        drag(&mut v, &mut d, &reg, &[start, end], mods);
        curve(&d, ExpressionKind::Pitch)[1].value - 1.5
    };
    let (plain, fine) = (moved(Modifiers::NONE), moved(shift()));
    assert!(
        plain > 5.0,
        "20 px is many semitones on the 48 scale: {plain}"
    );
    assert!((fine - plain / 10.0).abs() < 0.2, "{fine} vs {plain}");
    // Control: the dragged point lands on a whole semitone.
    let (mut v, mut d, reg, _, _) = setup();
    let start = v.expression_point_center(1).unwrap();
    drag(
        &mut v,
        &mut d,
        &reg,
        &[start, Point::new(start.x, start.y - 13.0)],
        ctrl(),
    );
    let value = curve(&d, ExpressionKind::Pitch)[1].value;
    assert!((value - value.round()).abs() < 1e-5, "{value}");
    assert_ne!(value, 1.5);
}

#[test]
fn alt_click_double_click_right_click_and_the_eraser_delete_a_point() {
    let delete = |how: &dyn Fn(&mut PianoRollView, &mut Document, &Registry, Point)| {
        let (mut v, mut d, reg, _, _) = setup();
        let before = d.doc().clone();
        let at = v.expression_point_center(1).unwrap();
        how(&mut v, &mut d, &reg, at);
        let c = curve(&d, ExpressionKind::Pitch);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(c.iter().all(|p| p.tick != PPQ / 8));
        d.undo().unwrap();
        assert_eq!(d.doc(), &before, "one undo step");
    };
    delete(&|v, d, reg, at| {
        send(v, d, reg, &down_mods(at, alt(), 1));
        send(v, d, reg, &up(at));
    });
    delete(&|v, d, reg, at| {
        send(v, d, reg, &down(at));
        send(v, d, reg, &up(at));
        send(v, d, reg, &down_mods(at, Modifiers::NONE, 2));
        send(v, d, reg, &up(at));
    });
    delete(&|v, d, reg, at| {
        send(v, d, reg, &right_down(at));
        send(v, d, reg, &right_up(at));
    });
    delete(&|v, d, reg, at| {
        v.set_tool(Tool::Eraser);
        click(v, d, reg, at);
    });
}

#[test]
fn a_rubber_band_selects_points_and_they_move_and_delete_together() {
    let (mut v, mut d, reg, _, _) = setup();
    let (_, _, len) = first_note(&d);
    let p0 = v.expression_point_center(0).unwrap();
    let p1 = v.expression_point_center(1).unwrap();
    let p2 = v.expression_point_center(2).unwrap();
    let plot = v.expression_plot().unwrap();
    // A band round the first two points, started on empty space.
    let corner_a = Point::new(p0.x - 4.0, plot.y0 + 1.0);
    let corner_b = Point::new(p1.x + 4.0, plot.y1 - 1.0);
    assert!(
        corner_b.x < p2.x - 8.0,
        "the band leaves the third point out"
    );
    let before = d.doc().clone();
    let a = drag(&mut v, &mut d, &reg, &[corner_a, corner_b], Modifiers::NONE);
    assert!(
        song_commands(&a).is_empty(),
        "a band changes nothing in the song"
    );
    assert_eq!(d.doc(), &before);
    assert_eq!(v.expression_selection(), vec![0, 1]);
    // Dragging one of them moves both, together, as one undo step.
    let start = v.expression_point_center(1).unwrap();
    drag(
        &mut v,
        &mut d,
        &reg,
        &[start, Point::new(start.x + 12.0, start.y)],
        Modifiers::NONE,
    );
    let c = curve(&d, ExpressionKind::Pitch);
    assert_eq!(c.len(), 3);
    let shift = c[0].tick;
    assert!(shift > 0 && shift < len / 4, "{c:?}");
    assert_eq!(c[1].tick - shift, PPQ / 8, "the group keeps its shape");
    assert_eq!(c[2].tick, PPQ / 4, "the third point stays");
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    // Delete removes the selected points, one undo step.
    v.sync(d.doc(), &reg);
    assert_eq!(v.expression_selection(), vec![0, 1]);
    let out = v.command("edit.delete", None, d.doc(), &reg);
    let CommandOutcome::Handled(actions) = out else {
        panic!("delete is the strip's");
    };
    apply(&mut v, &mut d, &reg, &actions);
    assert_eq!(curve(&d, ExpressionKind::Pitch), [pt(PPQ / 4, -2.0)]);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn shift_adds_to_the_selection_of_a_band_and_select_all_takes_every_point() {
    let (mut v, mut d, reg, _, _) = setup();
    let plot = v.expression_plot().unwrap();
    let p0 = v.expression_point_center(0).unwrap();
    let p2 = v.expression_point_center(2).unwrap();
    drag(
        &mut v,
        &mut d,
        &reg,
        &[
            Point::new(p0.x - 4.0, plot.y0 + 1.0),
            Point::new(p0.x + 4.0, plot.y1 - 1.0),
        ],
        Modifiers::NONE,
    );
    assert_eq!(v.expression_selection(), vec![0]);
    // A second band with Shift held on its moves adds.
    let start = Point::new(p2.x - 4.0, plot.y0 + 1.0);
    send(&mut v, &mut d, &reg, &down(start));
    send(
        &mut v,
        &mut d,
        &reg,
        &mv_with(Point::new(p2.x + 4.0, plot.y1 - 1.0), shift()),
    );
    send(
        &mut v,
        &mut d,
        &reg,
        &up(Point::new(p2.x + 4.0, plot.y1 - 1.0)),
    );
    // (the base is read when the band starts: Shift on the first move keeps the old points)
    assert_eq!(v.expression_selection(), vec![0, 2]);
    let out = v.command("edit.select_all", None, d.doc(), &reg);
    assert!(matches!(out, CommandOutcome::Handled(_)));
    assert_eq!(v.expression_selection(), vec![0, 1, 2]);
}

#[test]
fn a_freehand_stroke_with_the_pencil_draws_a_thinned_curve_as_one_undo_step() {
    let (mut v, mut d, reg, _, _) = setup();
    tab(&mut v, &mut d, &reg, ExpressionKind::Timbre);
    v.set_tool(Tool::Pencil);
    let (_, _, len) = first_note(&d);
    let before = d.doc().clone();
    // A straight rise sampled at every pixel, then a corner and a fall.
    let plot = v.expression_plot().unwrap();
    let mut path = Vec::new();
    let n = 60;
    for i in 0..=n {
        let f = f64::from(i) / f64::from(n);
        path.push(Point::new(
            plot.x0 + 20.0 + f * 100.0,
            plot.y1 - 2.0 - f * (plot.height() - 4.0),
        ));
    }
    for i in 1..=n {
        let f = f64::from(i) / f64::from(n);
        path.push(Point::new(
            plot.x0 + 120.0 + f * 60.0,
            plot.y0 + 2.0 + f * (plot.height() - 4.0),
        ));
    }
    let a = drag(&mut v, &mut d, &reg, &path, Modifiers::NONE);
    let c = curve(&d, ExpressionKind::Timbre);
    assert!(
        c.len() >= 3 && c.len() < 20,
        "thinned from {}: {c:?}",
        path.len()
    );
    assert!(c.windows(2).all(|w| w[0].tick < w[1].tick), "sorted");
    assert!(c.iter().all(|p| (0..=len).contains(&p.tick)));
    // The corner at the top survives.
    assert!(c.iter().any(|p| p.value > 0.9), "{c:?}");
    assert!(a.iter().any(|x| matches!(x, UiAction::EndMerge)));
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "one undo step for the whole stroke");
}

#[test]
fn escape_during_a_drag_puts_the_curve_back() {
    let (mut v, mut d, reg, _, _) = setup();
    let before = d.doc().clone();
    let start = v.expression_point_center(1).unwrap();
    send(&mut v, &mut d, &reg, &down(start));
    send(
        &mut v,
        &mut d,
        &reg,
        &mv(Point::new(start.x + 15.0, start.y - 10.0)),
    );
    assert_ne!(d.doc(), &before);
    let out = v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg);
    apply(&mut v, &mut d, &reg, &out);
    assert_eq!(
        curve(&d, ExpressionKind::Pitch),
        [pt(0, 0.0), pt(PPQ / 8, 1.5), pt(PPQ / 4, -2.0)]
    );
}

#[test]
fn a_full_curve_takes_no_new_point() {
    let (mut v, mut d, reg, tick, key) = setup();
    let (_, _, len) = first_note(&d);
    let mut e = NoteExpression::new(tick, key);
    e.timbre = (0..1024).map(|t| pt(t * (len / 1100), 0.5)).collect();
    let midi = MidiData {
        expression: vec![e],
        ..Default::default()
    };
    d.apply(
        Command::Song(SongCommand::SetClipMidi {
            clip: ClipId(1),
            midi,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    tab(&mut v, &mut d, &reg, ExpressionKind::Timbre);
    let before = d.doc().clone();
    // A spot between the points, away from every one of them: the click would add a point.
    let at = plot_at(&v, len, ExpressionKind::Timbre, len - 3, 0.95);
    let a = click(&mut v, &mut d, &reg, at);
    assert!(song_commands(&a).is_empty(), "the cap of 1024 points holds");
    assert_eq!(d.doc(), &before);
}

// ----- several notes ------------------------------------------------------------------------

#[test]
fn apply_copies_the_curve_to_the_other_selected_notes_scaled_to_each_length() {
    let (mut v, mut d, reg, _, _) = setup();
    let notes = {
        let c = d.doc().song.clip(ClipId(1)).unwrap().2.clone();
        let atomik_model::ClipContent::Notes(n) = &c.content else {
            panic!();
        };
        n.to_vec()
    };
    let (src_len, other) = (notes[0].length, notes[2]);
    v.set_selected_notes(&[0, 2, 1]);
    v.command("editor.edit_expression", None, d.doc(), &reg);
    let before = d.doc().clone();
    let at = v.expression_apply_center().expect("the apply button");
    let a = click(&mut v, &mut d, &reg, at);
    assert_eq!(song_commands(&a).len(), 1, "one command");
    let m = midi_of(&d);
    let copied = m.expression_of(other.tick, other.key).expect("copied");
    assert_eq!(copied.pitch.len(), 3);
    assert_eq!(
        copied.pitch[1].tick,
        (PPQ / 8) * other.length / src_len,
        "scaled to the other note's length"
    );
    assert_eq!(copied.pitch[1].value, 1.5);
    assert!(
        copied.pressure.is_empty(),
        "only the chosen curve is copied"
    );
    assert!(m.expression_of(notes[1].tick, notes[1].key).is_some());
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "one undo step");
}

#[test]
fn the_command_applies_the_chosen_tab_and_declines_with_one_note_selected() {
    let (mut v, mut d, reg, _, _) = setup();
    let out = v.command("editor.apply_expression_curve", None, d.doc(), &reg);
    assert!(matches!(out, CommandOutcome::Declined), "one note only");
    tab(&mut v, &mut d, &reg, ExpressionKind::Pressure);
    v.set_selected_notes(&[0, 1]);
    let CommandOutcome::Handled(actions) =
        v.command("editor.apply_expression_curve", None, d.doc(), &reg)
    else {
        panic!("two notes are selected");
    };
    apply(&mut v, &mut d, &reg, &actions);
    let (tick, key) = {
        let c = d.doc().song.clip(ClipId(1)).unwrap().2.clone();
        let atomik_model::ClipContent::Notes(n) = &c.content else {
            panic!();
        };
        (n[1].tick, n[1].key)
    };
    let e = midi_of(&d).expression_of(tick, key).cloned().unwrap();
    assert_eq!(e.pressure.len(), 2);
    assert!(e.pitch.is_empty());
}
