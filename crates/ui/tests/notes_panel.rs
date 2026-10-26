//! The Audio to Notes panel: its rows step and clamp, the rows that do not apply to the mode stay
//! put, EXTRACT needs a target, and the picture follows the values.

mod support;

use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::{
    DisplayList, Modifiers, NotesAction, NotesMode, NotesRow, NotesValues, NotesView, Theme,
    UiAction, UiEvent,
};
use support::down;

fn click(v: &mut NotesView, pos: Point) -> Vec<UiAction> {
    v.handle(&down(pos))
}

fn view() -> NotesView {
    NotesView::new(Theme::dark(), NotesValues::default())
}

#[test]
fn the_rows_read_their_defaults_and_step() {
    let mut v = view();
    assert_eq!(v.value_text(NotesRow::Mode), "Pitched (one voice)");
    assert_eq!(v.value_text(NotesRow::Sensitivity), "50 %");
    assert_eq!(v.value_text(NotesRow::MinNote), "60 ms");
    assert_eq!(v.value_text(NotesRow::Lowest), "C2");
    assert_eq!(v.value_text(NotesRow::Highest), "C7");
    assert_eq!(v.value_text(NotesRow::Drift), "70 cents");
    assert_eq!(v.value_text(NotesRow::Timing), "Free");
    assert_eq!(v.value_text(NotesRow::Target), "New track");
    let plus = v.plus_rect(NotesRow::Sensitivity).center();
    assert!(click(&mut v, plus).is_empty());
    assert_eq!(v.values().sensitivity, 55);
    let minus = v.minus_rect(NotesRow::MinNote).center();
    click(&mut v, minus);
    assert_eq!(v.values().min_note_ms, 45);
    v.step(NotesRow::Timing, 1);
    assert_eq!(v.value_text(NotesRow::Timing), "1/4");
    for _ in 0..10 {
        v.step(NotesRow::Timing, 1);
    }
    assert_eq!(v.value_text(NotesRow::Timing), "1/32");
    v.step(NotesRow::Target, 1);
    assert_eq!(v.value_text(NotesRow::Target), "Selected note track");
}

#[test]
fn values_clamp_and_the_range_keeps_its_order() {
    let mut v = view();
    for _ in 0..40 {
        v.step(NotesRow::Sensitivity, 1);
    }
    assert_eq!(v.values().sensitivity, 100);
    for _ in 0..40 {
        v.step(NotesRow::Sensitivity, -1);
    }
    assert_eq!(v.values().sensitivity, 0);
    for _ in 0..200 {
        v.step(NotesRow::Lowest, 1);
    }
    assert_eq!(
        v.values().key_lo,
        v.values().key_hi,
        "lowest stops at highest"
    );
    for _ in 0..200 {
        v.step(NotesRow::Highest, -1);
    }
    assert_eq!(v.values().key_hi, v.values().key_lo);
    for _ in 0..200 {
        v.step(NotesRow::Lowest, -1);
    }
    assert_eq!(v.values().key_lo, 0);
    assert_eq!(v.value_text(NotesRow::Lowest), "C-1");
    for _ in 0..200 {
        v.step(NotesRow::Highest, 1);
    }
    assert_eq!(v.values().key_hi, 127);
    for _ in 0..20 {
        v.step(NotesRow::Drift, 1);
        v.step(NotesRow::MinNote, 1);
    }
    assert_eq!((v.values().drift_cents, v.values().min_note_ms), (300, 400));
}

#[test]
fn rows_that_do_not_apply_to_the_mode_read_na_and_stay_put() {
    let mut v = view();
    assert_eq!(v.value_text(NotesRow::OpenHats), "n/a");
    v.step(NotesRow::OpenHats, 1);
    assert!(v.values().open_hats, "not changed while pitched");
    v.step(NotesRow::Mode, 1);
    assert_eq!(v.values().mode, NotesMode::Drums);
    assert_eq!(v.value_text(NotesRow::OpenHats), "On");
    for row in [
        NotesRow::MinNote,
        NotesRow::Lowest,
        NotesRow::Highest,
        NotesRow::Drift,
    ] {
        assert_eq!(v.value_text(row), "n/a");
        let before = v.values().clone();
        v.step(row, 1);
        assert_eq!(v.values(), &before);
    }
    v.step(NotesRow::OpenHats, 1);
    assert_eq!(v.value_text(NotesRow::OpenHats), "Off");
    v.step(NotesRow::Mode, -1);
    assert_eq!(v.values().mode, NotesMode::Pitched);
}

#[test]
fn modes_have_ids_for_scripts() {
    for m in [NotesMode::Pitched, NotesMode::Drums] {
        assert_eq!(NotesMode::from_id(m.id()), Some(m));
    }
    assert_eq!(NotesMode::from_id("vocal"), None);
}

#[test]
fn extract_needs_a_target_and_close_always_works() {
    let mut v = view();
    let apply = v.apply_rect().center();
    assert!(click(&mut v, apply).is_empty(), "nothing selected");
    v.set_target("Vocal.wav".into(), true);
    assert_eq!(
        click(&mut v, apply),
        vec![UiAction::AudioToNotes(NotesAction::Apply)]
    );
    let close = v.close_rect().center();
    assert_eq!(
        click(&mut v, close),
        vec![UiAction::AudioToNotes(NotesAction::Close)]
    );
    let row = v.row_rect(NotesRow::Sensitivity).center();
    v.handle(&UiEvent::Wheel {
        pos: row,
        delta: Vec2::new(0.0, 1.0),
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.values().sensitivity, 55);
}

#[test]
fn the_panel_paints_and_its_picture_changes_with_the_values() {
    let paint = |v: &NotesView| {
        let mut dl = DisplayList::new();
        v.paint(&mut dl);
        dl.to_json()
    };
    let mut v = view();
    v.set_target("Vocal.wav".into(), true);
    let a = paint(&v);
    assert!(a.contains("AUDIO TO NOTES") && a.contains("Vocal.wav") && a.contains("EXTRACT"));
    assert!(a.contains("Pitched (one voice)") && a.contains("50 %"));
    v.step(NotesRow::Mode, 1);
    let b = paint(&v);
    assert_ne!(b, a);
    assert!(b.contains("Drums"));
}

#[test]
fn audio_to_notes_is_a_menu_command_in_the_registry_without_a_key() {
    let r = atomik_ui::keymap::Registry::bundled();
    assert!(r.validate("sequencer.audio_to_notes", None));
    assert!(r.get("sequencer.audio_to_notes").unwrap().menu);
}

/// Presses on a value box, drags by `dy` pixels (up is negative) and releases.
fn drag_value(handle: &mut dyn FnMut(&UiEvent), at: Point, dy: f64) {
    let to = Point::new(at.x, at.y + dy);
    handle(&support::down(at));
    handle(&support::mv(to));
    handle(&support::up(to));
}

#[test]
fn dragging_a_value_steps_it_in_the_notes_silence_and_warp_panels() {
    use atomik_ui::{SilenceRow, SilenceValues, SilenceView, WarpRow, WarpValues, WarpView};
    let mut n = view();
    let at = n.value_rect(NotesRow::Sensitivity).center();
    drag_value(&mut |e| drop(n.handle(e)), at, -10.0);
    assert_eq!(
        n.values().sensitivity,
        60,
        "up, 5 px a step, 5 percent each"
    );
    assert!(!n.is_dragging());
    let at = n.value_rect(NotesRow::MinNote).center();
    drag_value(&mut |e| drop(n.handle(e)), at, 5.0);
    assert_eq!(n.values().min_note_ms, 45);

    let mut s = SilenceView::new(Theme::dark(), SilenceValues::default());
    let at = s.value_rect(SilenceRow::Threshold).center();
    drag_value(&mut |e| drop(s.handle(e)), at, -15.0);
    assert_eq!(s.values().threshold_db, -37);
    drag_value(&mut |e| drop(s.handle(e)), at, 5.0);
    assert_eq!(s.values().threshold_db, -38);

    let mut w = WarpView::new(Theme::dark(), WarpValues::default());
    let at = w.value_rect(WarpRow::Tempo).center();
    let before = w.values().bpm_tenths;
    drag_value(&mut |e| drop(w.handle(e)), at, -10.0);
    assert!(w.values().bpm_tenths > before);
    let up_to = w.values().bpm_tenths;
    drag_value(&mut |e| drop(w.handle(e)), at, 10.0);
    assert!(w.values().bpm_tenths < up_to);
}
